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
# Loot lists: Forever tries the "forever" branch, then "classic" (the default); quick in tests.
export WOWHEAD_MIN_GAP_MS=100
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
# Email (password resets, confirmations): a local SMTP sink that writes each
# mail to $MAIL_DIR as JSON (ci/smtp_sink.py); ci/auth_test.sh reads them.
export MAIL_DIR=/tmp/outhouse-mail
rm -rf "$MAIL_DIR"; mkdir -p "$MAIL_DIR"
python3 ci/smtp_sink.py 2525 "$MAIL_DIR" >/tmp/smtp-sink.log 2>&1 &
SMTP_PID=$!
export SMTP_HOST=127.0.0.1 SMTP_PORT=2525 SMTP_TLS=none MAIL_FROM="Outhouse <no-reply@outhouse.test>"
# The two-factor secret key (a throwaway for tests; real servers get one from deploy/setup.sh).
export AUTH_SECRET_KEY=ci-only-auth-secret-key-0123456789abcdef
# Child-safety reports alert the owner through the deploy kit's alert settings.
export ALERT_NTFY_URL=$PUSH/alert/safety-ntfy ALERT_WEBHOOK_URL=$PUSH/alert/safety-hook
# Where backup.sh would leave its status (read by the server status page).
STATUS_DIR=$(mktemp -d)
export BACKUP_STATUS_FILE=$STATUS_DIR/backup.json OUTHOUSE_VERSION=smoke-test
# The image detector sidecar (deploy/detector) in test mode: CI starts it as the
# "outhouse-detector" container on 127.0.0.1:8095 (see ci.yml). To run this
# script without Docker, set SMOKE_DETECTOR_CMD to start it another way, e.g.
#   SMOKE_DETECTOR_CMD="env DETECTOR_MODEL=/path/320n.onnx python3 deploy/detector/detector.py"
export DETECTOR_URL=${DETECTOR_URL:-http://127.0.0.1:8095}
DETECTOR_PID=""
detector_start() {
  if [ -n "${SMOKE_DETECTOR_CMD:-}" ]; then
    # shellcheck disable=SC2086
    DETECTOR_TEST_MODE=1 DETECTOR_BIND=127.0.0.1 DETECTOR_PORT=8095 DETECTOR_TEST_SLOW_SECS=4 $SMOKE_DETECTOR_CMD >/tmp/detector.log 2>&1 &
    DETECTOR_PID=$!
  else
    docker start outhouse-detector >/dev/null
  fi
  for _ in $(seq 1 90); do curl -fsS "$DETECTOR_URL/health" >/dev/null 2>&1 && return 0; sleep 1; done
  echo "the image detector didn't start"; docker logs outhouse-detector 2>&1 | tail -20 || true; return 1
}
detector_stop() {
  if [ -n "$DETECTOR_PID" ]; then kill "$DETECTOR_PID" 2>/dev/null || true; wait "$DETECTOR_PID" 2>/dev/null || true; DETECTOR_PID=""
  else docker stop -t 2 outhouse-detector >/dev/null; fi
  for _ in $(seq 1 20); do curl -fsS "$DETECTOR_URL/health" >/dev/null 2>&1 || return 0; sleep 0.5; done
}
if [ -n "${SMOKE_DETECTOR_CMD:-}" ]; then detector_start; fi
curl -fsS "$DETECTOR_URL/health" | jq -e '.ok and .test_mode' >/dev/null || { echo "the image detector (test mode) isn't running at $DETECTOR_URL"; exit 1; }
python3 ci/detector_images.py /tmp/dt >/dev/null
# IP retention runs on demand (POST /api/safety/ips/purge) only with this switch.
export OUTHOUSE_TEST_HOOKS=1
# Message requests: a small daily limit so the test can reach it (the default is 10).
export MESSAGE_REQUESTS_PER_DAY=4 MESSAGE_REQUEST_MAX_MESSAGES=3
# Gateway probe files from an earlier run would satisfy "wait for READY" before the new probe starts.
rm -f /tmp/gw-*.jsonl
./target/debug/outhouse-server >"$LOG" 2>&1 &
SERVER=$!
trap 'kill $SERVER $MOCK_PID $LK_PID $PUSH_PID $SMTP_PID $DETECTOR_PID 2>/dev/null || true' EXIT

for i in $(seq 1 60); do
  curl -fsS "$BASE/api/health" >/dev/null 2>&1 && break
  if ! kill -0 $SERVER 2>/dev/null; then echo "server exited:"; cat "$LOG"; exit 1; fi
  sleep 1
done
# The detector is switched on in its own section near the end (the older
# sections check pictures straight after uploading them).
psql "$DATABASE_URL" -qc "UPDATE site_settings SET detector_enabled = false"

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
A=$(call POST /api/auth/register '{"username":"alice","email":"alice@example.test","password":"password123","birth_date":"1990-01-01","display_name":"Alice"}')
TOKEN_A=$(jq -r .token <<<"$A"); ALICE=$(jq -r .user.id <<<"$A")
B=$(call POST /api/auth/register '{"username":"bob","email":"bob@example.test","password":"password123","birth_date":"1990-01-01","display_name":"Bob"}')
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
OUT=$(call POST /api/auth/register '{"username":"outsider","email":"outsider@example.test","password":"password123","birth_date":"1990-01-01"}' | jq -r .token)
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
trap 'kill $SERVER $BOT_PID $MOCK_PID $LK_PID $PUSH_PID $SMTP_PID $DETECTOR_PID 2>/dev/null || true' EXIT
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
C3=$(call POST /api/auth/register '{"username":"carl","email":"carl@example.test","password":"password123","birth_date":"1990-01-01","display_name":"Carl"}')
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
# Shares no outhouse with Alice and isn't her friend: a message request (see the
# message request sections near the end).
call POST /api/dms '{"user_ids":["'"$ALICE"'"]}' | jq -e '.request == "outgoing"' >/dev/null
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

step "raid composition: roles, classes, armour, buffs and dispels"
LC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"loot"}' | jq -r .id)
CE=$(call POST "/api/channels/$LC/events" '{"title":"Comp check","starts_at":"'"$START"'","game":"forever","max_tanks":1,"max_healers":3,"max_dps":3}' | jq -r .event.id)
call PUT "/api/events/$CE/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' >/dev/null
TOKEN=$TOKEN_B; call PUT "/api/events/$CE/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
TOKEN=$TOKEN_C; call PUT "/api/events/$CE/signup" '{"status":"tentative","role":"dps","class":"Mage","spec":"Frost"}' >/dev/null; TOKEN=$TOKEN_A
# The roster only (Carl is tentative): a Warrior and a Priest.
COMP=$(call GET "/api/events/$CE/composition")
jq -e '.era == "classic" and .counted == 2 and (.include_extra | not)
  and ([.roles[] | [.role, .count, .max, .missing]] == [["tank", 1, 1, 0], ["healer", 1, 3, 2], ["dps", 0, 3, 3]])
  and ([.classes[].class] == ["Priest", "Warrior"]) and ([.armour[].count] == [1, 0, 0, 1])
  and (.buffs[] | select(.key == "fortitude") | .covered and .providers == ["Bob"] and .kind == "buff")
  and (.buffs[] | select(.key == "battle_shout") | .covered and .providers == ["Ally"])
  and (.buffs[] | select(.key == "sunder_armor") | .covered and .kind == "debuff")
  and (.buffs[] | select(.key == "arcane_intellect") | .covered | not)
  and (.buffs[] | select(.key == "trueshot_aura") | (.covered | not) and .sources == ["Marksmanship Hunter"])
  and (.buffs[] | select(.key == "decurse") | (.covered | not) and .sources == ["Mage", "Druid"] and .kind == "dispel")
  and (.buffs[] | select(.key == "cure_disease") | .covered)
  and all(.buffs[]; .key != "bloodlust" and .key != "misery")
  and .missing_line == "Need: 2 healers · 3 DPS · No Druid (Mark of the Wild, battle res) · No Mage (Arcane Intellect) · No Paladin (Blessings) · No Shaman (Windfury) · No Warlock (Curses, Healthstones) · No decurse · No poison cure"' <<<"$COMP" >/dev/null \
  || { echo "odd composition: $COMP"; exit 1; }
# Counting tentative and bench too: Carl's Mage brings Arcane Intellect, Winter's Chill and decurse.
call GET "/api/events/$CE/composition?extra=1" | jq -e '.counted == 3 and .include_extra
  and (.buffs[] | select(.key == "arcane_intellect") | .providers == ["Carl"])
  and (.buffs[] | select(.key == "winters_chill") | .covered) and (.buffs[] | select(.key == "improved_scorch") | .covered | not)
  and (.missing | index("No decurse") | not) and (.missing | index("No Mage (Arcane Intellect)") | not) and (.missing | index("2 DPS"))' >/dev/null
# A TBC raid checks TBC buffs (Bloodlust, Misery, Drums as a note) and not Tranquilizing Shot.
call PATCH "/api/events/$CE" '{"game":"tbc"}' >/dev/null
call GET "/api/events/$CE/composition" | jq -e '.era == "tbc" and any(.buffs[]; .key == "bloodlust" and (.covered | not))
  and any(.buffs[]; .key == "drums" and .kind == "note" and (.covered | not)) and all(.buffs[]; .key != "tranq_shot")
  and (.missing_line | contains("No Shaman (Windfury, Bloodlust)"))' >/dev/null
# Outsiders can't look.
TOKEN=$OUT; fail_call GET "/api/events/$CE/composition"; TOKEN=$TOKEN_A

step "soft reserves: settings, limits, duplicates, permissions, lock, export"
fail_call POST "/api/channels/$LC/events" '{"title":"Too many","starts_at":"'"$START"'","soft_reserves":3}'
fail_call POST "/api/channels/$LC/events" '{"title":"Odd lock","starts_at":"'"$START"'","soft_reserves":1,"reserve_lock":"whenever"}'
SRE=$(call POST "/api/channels/$LC/events" '{"title":"MC with reserves","starts_at":"'"$START"'","game":"forever","soft_reserves":1,"reserve_lock":"manual","loot_instance":"Molten Core"}' | jq -r .event.id)
call GET "/api/events/$SRE" | jq -e '.soft_reserves == 1 and .reserve_lock == "manual" and .loot_instance == "mc" and .loot_instance_name == "Molten Core" and (.reserves_locked | not) and .reserves == [] and .reserve_lock_override == null' >/dev/null
call GET "/api/events/$SRE/loot-table" | jq -e '.instance == "mc" and .instance_name == "Molten Core" and (.items | length >= 8)
  and any(.items[]; .item_id == 17076 and .name == "Bonereaver'"'"'s Edge" and .boss == "Ragnaros" and .quality == 4 and (.custom | not))' >/dev/null
call GET "/api/guilds/$GUILD/loot-instances" | jq -e 'length >= 16 and any(.[]; .key == "naxx" and .era == "classic" and .builtin) and any(.[]; .key == "kara" and .era == "tbc" and .size == 10)' >/dev/null
TOKEN=$OUT; fail_call GET "/api/events/$SRE/loot-table"; fail_call GET "/api/guilds/$GUILD/loot-instances"; TOKEN=$TOKEN_A
# Off for the composition raid; only people signed up can reserve.
TOKEN=$TOKEN_B
fail_call PUT "/api/events/$CE/reserves" '{"item_id":17076}'
fail_call PUT "/api/events/$SRE/reserves" '{"item_id":17076}'
call PUT "/api/events/$SRE/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
call PUT "/api/events/$SRE/reserves" '{"item_id":17076}' | jq -e --arg b "$BOB" '.event.reserves | length == 1 and .[0].user_id == $b and .[0].item_id == 17076 and .[0].item_name == "Bonereaver'"'"'s Edge" and .[0].quality == 4' >/dev/null
# One each: a second is refused, and the same item twice too.
fail_call PUT "/api/events/$SRE/reserves" '{"item_id":18814}'; grep -q "used your soft reserve" /tmp/body
fail_call PUT "/api/events/$SRE/reserves" '{"item_id":17076}'; grep -q "already reserved" /tmp/body
# Bob doesn't run this raid.
fail_call PUT "/api/events/$SRE/reserves/lock" '{"locked":true}'
fail_call GET "/api/events/$SRE/reserves/export"
fail_call POST "/api/events/$SRE/loot-items" '{"query":"19019"}'
TOKEN=$TOKEN_A
# Two each now: by name from the list, then a third is refused.
call PATCH "/api/events/$SRE" '{"soft_reserves":2}' | jq -e '.event.soft_reserves == 2' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/events/$SRE/reserves" '{"query":"leggings of transcendence"}' | jq -e '[.event.reserves[].item_id] == [17076, 16922]' >/dev/null
fail_call PUT "/api/events/$SRE/reserves" '{"item_id":18814}'
TOKEN=$TOKEN_C
call PUT "/api/events/$SRE/signup" '{"status":"bench","role":"dps","class":"Rogue","spec":"Combat"}' >/dev/null
# Any item by Wowhead link (looked up), and a WoW: Forever item from Battle.net when Wowhead hasn't got it.
call PUT "/api/events/$SRE/reserves" '{"query":"https://www.wowhead.com/classic/item=19019/thunderfury"}' \
  | jq -e '.event.reserves[-1] | .item_id == 19019 and .item_name == "Thunderfury, Blessed Blade of the Windseeker" and .quality == 5' >/dev/null
fail_call PUT "/api/events/$SRE/reserves" '{"query":"zzznotanitem"}'; grep -q "find that item" /tmp/body
call DELETE "/api/events/$SRE/reserves/19019" >/dev/null
call PUT "/api/events/$SRE/reserves" '{"query":"Skyborne Sash"}' | jq -e '.event.reserves[-1] | .item_id == 990001 and .item_name == "Skyborne Sash" and .quality == 4' >/dev/null
call PUT "/api/events/$SRE/reserves" '{"item_id":17076}' >/dev/null
# Carl can't take Bob's away.
fail_call DELETE "/api/events/$SRE/reserves/17076?user_id=$BOB"
TOKEN=$OUT; fail_call PUT "/api/events/$SRE/reserves" '{"item_id":17076}'; TOKEN=$TOKEN_A
# Everyone sees who reserved what (two on Bonereaver's Edge).
TOKEN=$TOKEN_B; call GET "/api/events/$SRE" | jq -e '[.reserves[] | select(.item_id == 17076)] | length == 2' >/dev/null
call GET "/api/events/$SRE/loot-table" | jq -e 'any(.items[]; .item_id == 990001 and .name == "Skyborne Sash")' >/dev/null; TOKEN=$TOKEN_A
# Locking by hand: members are stuck, the raid leader can still remove one.
call PUT "/api/events/$SRE/reserves/lock" '{"locked":true}' | jq -e '.event.reserves_locked and .event.reserve_lock_override == true' >/dev/null
TOKEN=$TOKEN_B
fail_call DELETE "/api/events/$SRE/reserves/16922"; grep -q "locked" /tmp/body
TOKEN=$TOKEN_C; fail_call PUT "/api/events/$SRE/reserves" '{"item_id":18814}'; TOKEN=$TOKEN_A
call DELETE "/api/events/$SRE/reserves/17076?user_id=$CARL" | jq -e --arg c "$CARL" '[.event.reserves[] | select(.user_id == $c)] | map(.item_id) == [990001]' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=loot_reserve_remove" | jq -e '.entries[0].details | contains("Carl") and contains("Bonereaver") and contains("MC with reserves")' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=loot_reserves_lock" | jq -e '.entries[0].details | startswith("Locked soft reserves")' >/dev/null
call PUT "/api/events/$SRE/reserves/lock" '{"locked":false}' | jq -e '(.event.reserves_locked | not) and .event.reserve_lock_override == false' >/dev/null
call PUT "/api/events/$SRE/reserves/lock" '{"locked":null}' | jq -e '(.event.reserves_locked | not) and .event.reserve_lock_override == null' >/dev/null
# Export: softres.it CSV (one row per reserve) and "Item — Player, Player".
EXP=$(call GET "/api/events/$SRE/reserves/export")
jq -e '(.csv | startswith("Item Name,Item ID,From,Raider Name,Raider Class,Raider Spec,Raider Note,Extra Reserves,Date\nBonereaver'"'"'s Edge,17076,Ragnaros,Bob,Priest,Holy,,0,"))
  and (.csv | split("\n") | map(select(length > 0)) | length == 4)
  and (.text | split("\n")) == ["Bonereaver'"'"'s Edge — Bob", "Leggings of Transcendence — Bob", "Skyborne Sash — Carl"]' <<<"$EXP" >/dev/null || { echo "odd export: $EXP"; exit 1; }
# Back to one each: people keep their first.
call PATCH "/api/events/$SRE" '{"soft_reserves":1}' | jq -e --arg b "$BOB" '[.event.reserves[] | select(.user_id == $b) | .item_id] == [17076]' >/dev/null
# Locked at the sign-up deadline (the default): too late for Bob, even when added by the leader.
SOONISH=$(date -u -d '+30 minutes' +%Y-%m-%dT%H:%M:%SZ)
DLE=$(call POST "/api/channels/$LC/events" '{"title":"Deadline gone","starts_at":"'"$SOONISH"'","signup_deadline_minutes":60,"soft_reserves":1,"loot_instance":"ony"}' | jq -r .event.id)
call GET "/api/events/$DLE" | jq -e '.reserves_locked and .reserve_lock == "deadline" and .loot_instance_name == "Onyxia'"'"'s Lair"' >/dev/null
call PUT "/api/events/$DLE/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat","user_id":"'"$BOB"'"}' >/dev/null
TOKEN=$TOKEN_B; fail_call PUT "/api/events/$DLE/reserves" '{"item_id":17078}'; grep -q "locked" /tmp/body; TOKEN=$TOKEN_A
# The raid's own list: leaders add items (by ID, link or name) and remove the ones they added.
call POST "/api/events/$SRE/loot-items" '{"query":"https://www.wowhead.com/classic/item=19019"}' | jq -e 'any(.items[]; .item_id == 19019 and .custom and .quality == 5)' >/dev/null
fail_call POST "/api/events/$SRE/loot-items" '{"query":"19019"}'
fail_call POST "/api/events/$SRE/loot-items" '{"query":"17076"}'
fail_call DELETE "/api/events/$SRE/loot-items/17076"
call DELETE "/api/events/$SRE/loot-items/19019" | jq -e 'all(.items[]; .item_id != 19019)' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=loot_item_add" | jq -e '.entries[0].details | contains("Thunderfury") and contains("Molten Core")' >/dev/null
# Item look-ups for the pickers.
call GET "/api/wow/items?q=thunderfury" | jq -e '.item_id == 19019 and .quality == 5 and .icon_url == "/api/wow/item-icons/19019" and (.tooltip | contains("Binds when picked up")) and (.url | endswith("/item=19019"))' >/dev/null
call GET "/api/wow/items/16922" | jq -e '.name == "Leggings of Transcendence" and .quality == 4' >/dev/null
call GET "/api/wow/items/17204" | jq -e '.name == "Eye of Sulfuras" and .quality == 5' >/dev/null
fail_call GET "/api/wow/items?q="
fail_call GET "/api/wow/items/0"
TOKEN=""; fail_call GET "/api/wow/items/16922"; TOKEN=$TOKEN_A
# Repeating raids carry their reserve settings, also into a series split off with new days.
LF=$(date -u -d '+1 day' +%Y-%m-%dT21:00:00Z); LD=$(( $(date -u -d '+1 day' +%u) - 1 ))
LS=$(call POST "/api/channels/$LC/events" '{"title":"BWL reserves","starts_at":"'"$LF"'","post_rule":{"mode":"now"},"soft_reserves":2,"loot_instance":"bwl","repeat":{"weekdays":['"$LD"'],"every_weeks":1,"count":6,"time_zone":"UTC"}}')
LSER=$(jq -r .event.series_id <<<"$LS")
pevents "$LSER" | jq -e 'length >= 4 and all(.[]; .soft_reserves == 2 and .loot_instance == "bwl")' >/dev/null
L1=$(pevents "$LSER" | jq -r '.[1].id'); L1START=$(pevents "$LSER" | jq -r '.[1].starts_at')
LNEW=$(call PATCH "/api/events/$L1" '{"scope":"future","starts_at":"'"$L1START"'","repeat":{"weekdays":['"$LD"'],"every_weeks":2,"count":3,"time_zone":"UTC"}}' | jq -r .event.series_id)
[ "$LNEW" != "$LSER" ]
pevents "$LNEW" | jq -e 'length >= 2 and all(.[]; .soft_reserves == 2 and .loot_instance == "bwl")' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT soft_reserves || loot_instance FROM event_series WHERE id = $LNEW")" = 2bwl ]
# "This and future": the first raid keeps its reserves, later ones turn them off.
call PATCH "/api/events/$L1" '{"scope":"future","soft_reserves":0,"loot_instance":"My New Raid"}' | jq -e '.event.soft_reserves == 0 and .event.loot_instance == "My New Raid" and .event.loot_instance_name == "My New Raid"' >/dev/null
pevents "$LNEW" | jq -e 'all(.[]; .soft_reserves == 0)' >/dev/null
pevents "$LSER" | jq -e '.[0].soft_reserves == 2' >/dev/null
call GET "/api/guilds/$GUILD/loot-instances" | jq -e 'any(.[]; .key == "My New Raid" and (.builtin | not))' >/dev/null

step "soft-reserve exports: softres.it CSV and the Gargul import string"
GXE=$(call POST "/api/channels/$LC/events" '{"title":"Gargul night","description":"First line\nSecond line","starts_at":"'"$START"'","game":"forever","soft_reserves":2,"reserve_lock":"manual","loot_instance":"Molten Core"}' | jq -r .event.id)
TOKEN=$TOKEN_B
GCH=$(call POST /api/users/@me/characters '{"name":"Danité","last_name":"Shambulàncè","class":"Shaman","level":60}' | jq -r .id)
call PUT "/api/events/$GXE/signup" '{"status":"yes","role":"dps","class":"Shaman","spec":"elemental","note":"say \"hi\", ok","character_id":"'"$GCH"'"}' \
  | jq -e '.event.signups[0].character.full_name == "Danité Shambulàncè"' >/dev/null
call PUT "/api/events/$GXE/reserves" '{"query":"https://www.wowhead.com/classic/item=19019/thunderfury"}' >/dev/null
call PUT "/api/events/$GXE/reserves" '{"item_id":17076}' >/dev/null
# Only raid leaders export.
fail_call GET "/api/events/$GXE/softres/gargul"
fail_call GET "/api/events/$GXE/reserves/export"
TOKEN=$TOKEN_C
call PUT "/api/events/$GXE/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat"}' >/dev/null
call PUT "/api/events/$GXE/reserves" '{"item_id":17076}' >/dev/null
TOKEN=$OUT; fail_call GET "/api/events/$GXE/softres/gargul"; fail_call GET "/api/events/$GXE/reserves/export?names=first"; TOKEN=$TOKEN_A
# softres.it's CSV: its header, RFC 4180 quoting, UTF-8 names, by raider, the boss from the loot list, UTC times.
GEXP=$(call GET "/api/events/$GXE/reserves/export")
jq -e '(.csv | split("\n")) as $l | ($l | length == 5) and $l[4] == ""
  and $l[0] == "Item Name,Item ID,From,Raider Name,Raider Class,Raider Spec,Raider Note,Extra Reserves,Date"
  and ($l[1] | test("^Bonereaver'"'"'s Edge,17076,Ragnaros,Carl,Rogue,Combat,,0,[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}$"))
  and ($l[2] | test("^\"Thunderfury, Blessed Blade of the Windseeker\",19019,[^,]*,Danité Shambulàncè,Shaman,Elemental,\"say \"\"hi\"\", ok\",0,[0-9-]{10} [0-9:]{8}$"))
  and ($l[3] | startswith("Bonereaver'"'"'s Edge,17076,Ragnaros,Danité Shambulàncè,Shaman,Elemental,\"say \"\"hi\"\", ok\",0,"))
  and (.text | split("\n")) == ["Bonereaver'"'"'s Edge — Danité Shambulàncè, Carl", "Thunderfury, Blessed Blade of the Windseeker — Danité Shambulàncè"]' <<<"$GEXP" >/dev/null \
  || { echo "odd softres CSV: $GEXP"; exit 1; }
call GET "/api/events/$GXE/reserves/export?names=first" | jq -e '.csv | contains(",Danité,Shaman,") and (contains("Shambul") | not)' >/dev/null
# Gargul: base64(zlib(JSON)), escaped like softres.it (\/ and \uXXXX), one entry per raider.
gargul_json() { python3 -c 'import sys,json,base64,zlib; print(zlib.decompress(base64.b64decode(json.load(sys.stdin)["text"], validate=True)).decode("ascii"))'; }
GG=$(call GET "/api/events/$GXE/softres/gargul")
jq -e '.raiders == 2 and .reserves == 3' <<<"$GG" >/dev/null
GJ=$(gargul_json <<<"$GG")
[[ "$GJ" == *'"name":"Danit\u00e9 Shambul\u00e0nc\u00e8"'* ]] || { echo "unescaped Gargul JSON: $GJ"; exit 1; }
[[ "$GJ" == *':\/\/'* && "$GJ" != *'://'* ]] || { echo "slashes not escaped: $GJ"; exit 1; }
[[ "$GJ" == '{"metadata":{"id":"'* ]] || { echo "odd Gargul order: $GJ"; exit 1; }
jq -e --arg e "$GXE" '(.metadata | keys_unsorted) == ["id","url","instances","hidden","created_at","updated_at","raidStartsAt","lockedAt","discordUrl","note","bonusrolls_consumed_on_use"]
  and (.metadata.id | test("^[0-9a-zA-Z]{8}$")) and (.metadata.url | test("/app\\?guild=[0-9]+&channel=[0-9]+&event=" + $e + "$"))
  and .metadata.instances == ["moltencore"] and (.metadata.hidden | not) and .metadata.lockedAt == null and .metadata.discordUrl == ""
  and .metadata.note == "First line" and (.metadata.bonusrolls_consumed_on_use | not)
  and (.metadata.created_at | type == "number") and .metadata.updated_at >= .metadata.created_at and (.metadata.raidStartsAt | type == "number")
  and .softreserves == [
    {"name":"Carl","class":"rogue","note":null,"items":[{"id":17076,"note":"","order":0}]},
    {"name":"Danité Shambulàncè","class":"shaman","note":"say \"hi\", ok","items":[{"id":19019,"note":"","order":0},{"id":17076,"note":"","order":1}]}]
  and .hardreserves == [] and .bonusrolls == []' <<<"$GJ" >/dev/null || { echo "odd Gargul export: $GJ"; exit 1; }
# Same raid, same ID; first names only; locked reserves say when.
call PUT "/api/events/$GXE/reserves/lock" '{"locked":true}' >/dev/null
GJ2=$(call GET "/api/events/$GXE/softres/gargul?names=first" | gargul_json)
jq -e --argjson a "$GJ" '.metadata.id == $a.metadata.id and (.metadata.lockedAt | type == "number")
  and [.softreserves[].name] == ["Carl", "Danité"]' <<<"$GJ2" >/dev/null || { echo "odd Gargul export (first names): $GJ2"; exit 1; }
call PUT "/api/events/$GXE/reserves/lock" '{"locked":null}' >/dev/null

step "loot won and the loot log"
TOKEN=$OUT; OUTSIDER_ID=$(call GET /api/users/@me | jq -r .id); TOKEN=$TOKEN_A
# Not before the raid starts.
fail_call POST "/api/events/$SRE/loot" '{"user_id":"'"$BOB"'","item_id":17076}'; grep -q "once the raid has started" /tmp/body
LPAST=$(date -u -d '-40 minutes' +%Y-%m-%dT%H:%M:%SZ)
LP=$(call POST "/api/channels/$LC/events" '{"title":"Ony tonight","starts_at":"'"$LPAST"'","soft_reserves":1,"reserve_lock":"manual","loot_instance":"ony"}' | jq -r .event.id)
TOKEN=$TOKEN_B
call PUT "/api/events/$LP/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
call PUT "/api/events/$LP/reserves" '{"item_id":17078}' >/dev/null
fail_call POST "/api/events/$LP/loot" '{"user_id":"'"$BOB"'","item_id":17078}'
TOKEN=$TOKEN_A
call POST "/api/events/$LP/loot" '{"user_id":"'"$BOB"'","item_id":17078}' | jq -e --arg b "$BOB" '.user_id == $b and .item_name == "Sapphiron Drape" and .reserved and .raid_title == "Ony tonight" and .instance_name == "Onyxia'"'"'s Lair" and .display_name == "Bob"' >/dev/null
call POST "/api/events/$LP/loot" '{"user_id":"'"$CARL"'","query":"Thunderfury","note":"off-spec"}' | jq -e '.item_id == 19019 and (.reserved | not) and .note == "off-spec"' >/dev/null
fail_call POST "/api/events/$LP/loot" '{"user_id":"'"$OUTSIDER_ID"'","item_id":17078}'
fail_call POST "/api/events/$LP/loot" '{"user_id":"'"$CARL"'","query":"zzznotanitem"}'
call GET "/api/events/$LP/loot" | jq -e 'length == 2 and .[0].item_id == 17078 and .[1].item_id == 19019' >/dev/null
# The outhouse's log: newest first, filters, and on Bob's profile card.
call GET "/api/guilds/$GUILD/loot" | jq -e '(.entries | length == 2) and .entries[0].item_id == 19019 and (.has_more | not)' >/dev/null
call GET "/api/guilds/$GUILD/loot?user=$BOB&limit=5" | jq -e '.entries | length == 1 and .[0].item_name == "Sapphiron Drape"' >/dev/null
call GET "/api/guilds/$GUILD/loot?q=thunder" | jq -e '.entries | length == 1 and .[0].display_name == "Carl"' >/dev/null
call GET "/api/guilds/$GUILD/loot?instance=ony&limit=1" | jq -e '(.entries | length == 1) and .has_more' >/dev/null
FIRSTAW=$(call GET "/api/guilds/$GUILD/loot?limit=1" | jq -r '.entries[0].id')
call GET "/api/guilds/$GUILD/loot?before=$FIRSTAW" | jq -e '.entries | length == 1 and .[0].item_id == 17078' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/loot" | jq -e '.entries | length == 2' >/dev/null
fail_call DELETE "/api/loot/$FIRSTAW"; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/loot"; fail_call GET "/api/events/$LP/loot"; TOKEN=$TOKEN_A
# An 18+ channel's loot stays out of sight until you've said you're 18+.
call PATCH "/api/channels/$LC" '{"content_mode":"nsfw"}' >/dev/null
TOKEN=$TOKEN_B
call GET "/api/guilds/$GUILD/loot" | jq -e '.entries == []' >/dev/null
fail_call GET "/api/events/$LP/loot"; fail_call GET "/api/events/$CE/composition"; fail_call PUT "/api/events/$SRE/reserves" '{"item_id":18814}'
TOKEN=$TOKEN_A
call PATCH "/api/channels/$LC" '{"content_mode":"default"}' >/dev/null
call DELETE "/api/loot/$FIRSTAW" >/dev/null
call GET "/api/guilds/$GUILD/loot" | jq -e '.entries | length == 1' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=loot_award" | jq -e '.entries | length == 2 and (.[0].details | contains("Thunderfury") and contains("Carl"))' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=loot_award_remove" | jq -e '.entries | length == 1' >/dev/null
step "Wowhead loot lists: parsing, bosses, filters, caching, refresh, imports, branches, Wowhead down"
hits() { curl -fsS "$MOCK/__hits" | jq -r --arg p "$1" '.[$p] // 0'; }
# Molten Core's full list from its zone page (a trimmed copy of the real one,
# reached through Wowhead's redirect to its full address): Classic Era only
# (no Season of Discovery copies), Majordomo's loot from his chest's page,
# loot several bosses share in its own section. No "forever" branch (the
# mock answers 403 like wowhead.com), so WoW: Forever falls back to Classic.
MCT=$(call GET "/api/events/$SRE/loot-table")
echo "$MCT" | head -c 300; echo
jq -e '(.items | length) >= 40 and (.offline | not) and (.refreshing | not)
  and .item_branch == "classic" and (.item_base | endswith("/classic"))
  and .bosses == ["Lucifron", "Magmadar", "Gehennas", "Garr", "Shazzrah", "Baron Geddon", "Sulfuron Harbinger", "Golemagg the Incinerator", "Majordomo Executus", "Ragnaros", "Several bosses"]
  and ([.items[] | select(.item_id >= 200000 and .item_id < 900000)] | length == 0)
  and ([.items[] | select(.name == "Eye of Sulfuras")] | length == 1)
  and any(.items[]; .item_id == 17076 and .name == "Bonereaver'"'"'s Edge" and .boss == "Ragnaros" and .slot == "Two-Hand Sword" and .category == "weapon" and .icon == "inv_sword_12" and .classes == 7)
  and any(.items[]; .item_id == 16805 and .name == "Felheart Gloves" and .quality == 4 and .armour == "cloth" and .classes == 256)
  and any(.items[]; .item_id == 16922 and .slot == "Cloth Legs" and .classes == 16)
  and any(.items[]; .item_id == 18814 and .category == "neck" and .classes == 1503)
  and any(.items[]; .item_id == 17072 and .slot == "Gun" and .category == "weapon")
  and any(.items[]; .item_id == 18821 and .boss == "Several bosses" and (.trash | not))
  and any(.items[]; .item_id == 16799 and .trash and .boss == "")
  and any(.items[]; .item_id == 17010 and .trash and .boss == "")
  and any(.items[]; .item_id == 99901 and .quality == 0 and .trash)
  and any(.items[]; .item_id == 18646 and .boss == "Majordomo Executus" and (.trash | not) and .chance > 48 and .chance < 49)
  and any(.sources[]; .builtin and .kind == "zone" and .id == 2717 and .branch == "classic" and .status == "ok" and .name == "Molten Core" and .items >= 40)' <<<"$MCT" >/dev/null || { echo "$MCT" | jq -c '{bosses, sources, n: (.items | length), b: [.items[] | select(.item_id == 17076 or .item_id == 16805 or .item_id == 18646 or .item_id == 18821)]}'; exit 1; }
[ "$(hits /classic/object=179703)" = 1 ]
# Zone pages have no drop chances: the bosses' own pages are read slowly in
# the background for them (the mock only has Ragnaros's).
for i in $(seq 1 60); do
  call GET "/api/events/$SRE/loot-table" | jq -e 'any(.items[]; .item_id == 17076 and .chance == 12.5)' >/dev/null && break
  [ "$i" = 60 ] && { echo "no drop chance from Ragnaros's page"; curl -sS "$MOCK/__hits"; exit 1; }
  sleep 1
done
curl -fsS "$MOCK/__hits"; echo
# Cached: reading it again doesn't touch Wowhead; the missing branch was tried once.
[ "$(hits /classic/zone=2717)" = 1 ] && [ "$(hits /forever/zone=2717)" = 1 ]
call GET "/api/events/$SRE/loot-table" >/dev/null
[ "$(hits /classic/zone=2717)" = 1 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT status FROM wowhead_lists WHERE branch = 'forever' AND kind = 'zone' AND source_id = 2717")" = missing ]
# Refresh from Wowhead: raid leaders only, not twice in a minute; a week-old list is read again.
TOKEN=$TOKEN_B; fail_call POST "/api/events/$SRE/loot-table/refresh"; TOKEN=$TOKEN_A
call POST "/api/events/$SRE/loot-table/refresh" >/dev/null
[ "$(hits /classic/zone=2717)" = 1 ]
psql "$DATABASE_URL" -qc "UPDATE wowhead_lists SET fetched_at = fetched_at - interval '2 minutes' WHERE kind = 'zone' AND source_id = 2717"
call POST "/api/events/$SRE/loot-table/refresh" | jq -e '(.items | length) >= 30 and (.refreshing | not)' >/dev/null
[ "$(hits /classic/zone=2717)" = 2 ]
psql "$DATABASE_URL" -qc "UPDATE wowhead_lists SET ok_at = now() - interval '8 days', fetched_at = now() - interval '8 days' WHERE branch = 'classic' AND kind = 'zone' AND source_id = 2717"
call GET "/api/events/$SRE/loot-table" >/dev/null
[ "$(hits /classic/zone=2717)" = 3 ]
# Wowhead down: the saved list stays (marked), refresh says so.
curl -fsS "$MOCK/__down?on=1" >/dev/null
psql "$DATABASE_URL" -qc "UPDATE wowhead_lists SET fetched_at = fetched_at - interval '2 minutes' WHERE kind = 'zone' AND source_id = 2717"
call POST "/api/events/$SRE/loot-table/refresh" | jq -e '(.items | length) >= 30 and .sources[0].status == "error" and (.offline | not)' >/dev/null
call GET "/api/wow/items/17110" | jq -e '.name == "Seal of the Archmagus" and .quality == 4 and (.url | endswith("/item=17110"))' >/dev/null
curl -fsS "$MOCK/__down?on=0" >/dev/null
# Back up: tried again 10 minutes later.
psql "$DATABASE_URL" -qc "UPDATE wowhead_lists SET fetched_at = fetched_at - interval '11 minutes' WHERE kind = 'zone' AND source_id = 2717"
call GET "/api/events/$SRE/loot-table" | jq -e '.sources[0].status == "ok" and .sources[0].branch == "classic"' >/dev/null
# A raid Wowhead never answered for (the mock's Naxxramas is "down"): the built-in short list.
NX=$(call POST "/api/channels/$LC/events" '{"title":"Naxx night","starts_at":"'"$START"'","soft_reserves":1,"loot_instance":"naxx"}' | jq -r .event.id)
call GET "/api/events/$NX/loot-table" | jq -e '.offline and .sources[0].status == "error" and any(.items[]; .item_id == 22691 and .boss == "The Four Horsemen")' >/dev/null
# Onyxia: the zone page has no drops, so each boss's page is read (drop chances too).
call GET "/api/events/$LP/loot-table" | jq -e '.bosses == ["Onyxia"] and .item_branch == "classic"
  and any(.items[]; .item_id == 17078 and .boss == "Onyxia" and .chance == 13 and .category == "back" and (.trash | not))
  and any(.items[]; .item_id == 17067 and .slot == "Held In Off-hand")
  and .sources[0].name == "Onyxia'"'"'s Lair"' >/dev/null
[ "$(hits /classic/npc=10184)" = 1 ] && [ "$(hits /forever/zone=2159)" = 1 ]
# The Burning Crusade: the tbc branch, its own data set for tooltips.
KZ=$(call POST "/api/channels/$LC/events" '{"title":"Kara","starts_at":"'"$START"'","game":"tbc","soft_reserves":1,"loot_instance":"kara"}' | jq -r .event.id)
# Attumen and Midnight are one boss (most of his loot is only on his own
# page), Julianne, the Wolf and the Crone are the Opera event, the Chess
# event's loot is in its chest; rare spawns after the bosses.
KZT=$(call GET "/api/events/$KZ/loot-table")
jq -e '.item_branch == "tbc" and .bosses[0:6] == ["Attumen the Huntsman", "Opera Event", "Shade of Aran", "Chess Event", "Prince Malchezaar", "Hyakiss the Lurker"]
  and any(.items[]; .item_id == 30480 and .slot == "Mount" and .category == "other" and .boss == "Attumen the Huntsman")
  and any(.items[]; .item_id == 28453 and .boss == "Attumen the Huntsman" and (.trash | not) and .chance == 9.22)
  and any(.items[]; .item_id == 28572 and .boss == "Opera Event")
  and any(.items[]; .item_id == 28749 and .boss == "Chess Event" and .chance > 16)
  and any(.items[]; .item_id == 21903 and .trash)
  and all(.items[]; .item_id != 21877)
  and any(.items[]; .item_id == 28785 and .category == "trinket")' <<<"$KZT" >/dev/null || { echo "$KZT" | jq -c '{bosses, n: (.items | length), i: [.items[] | {item_id, boss, trash, chance}]}'; exit 1; }
# The Opera event's shared pool (a boss drop naming no boss on the zone
# page) is the Opera event's once its bosses' pages have been read.
for i in $(seq 1 60); do
  call GET "/api/events/$KZ/loot-table" | jq -e 'any(.items[]; .item_id == 28590 and .boss == "Opera Event" and .chance == 14.28) and any(.items[]; .item_id == 28581 and .chance == 20.63)' >/dev/null && break
  [ "$i" = 60 ] && { echo "the Opera event's shared loot wasn't given to it"; call GET "/api/events/$KZ/loot-table" | jq -c '[.items[] | {item_id, boss, chance}]'; exit 1; }
  sleep 1
done
call GET "/api/wow/items/28770?branch=tbc" | jq -e '.name == "Nathrezim Mindblade" and (.url | endswith("/tbc/item=28770")) and any(.lines[]; .left == "Drops from Prince Malchezaar")' >/dev/null
# Tooltips as Wowhead shows them: two columns and colours.
call GET "/api/wow/items/17076" | jq -e 'any(.lines[]; .left == "Two-Hand" and .right == "Sword") and any(.lines[]; .q == "q2" and (.left | startswith("Chance on hit")))' >/dev/null
# Icons through our server (no sign-in for <img>), kept after the first fetch.
[ "$(curl -s -o /tmp/icon -w '%{http_code} %{content_type}' "$BASE/api/wow/item-icons/17076")" = "200 image/png" ]
curl -fsS -o /dev/null "$BASE/api/wow/item-icons/17076"
[ "$(hits /icons/inv_sword_12.jpg)" = 1 ]
curl -sI "$BASE/api/wow/item-icons/17076" | grep -qi "cache-control: public"
[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/wow/item-icons/99901")" = 404 ]   # Wowhead has no picture
[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/wow/item-icons/4242")" = 404 ]    # not an item we know
[ "$(hits /tooltip/item/4242)" = 0 ]
[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/wow/item-icons/abc")" = 404 ]
# Importing a list by pasting a Wowhead link: a boss (NPC), a chest (object), a new Forever raid's zone.
TOKEN=$TOKEN_B; fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/npc=12397/lord-kazzak"}'; TOKEN=$TOKEN_A
call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/npc=12397/lord-kazzak"}' \
  | jq -e 'any(.items[]; .item_id == 17111 and .boss == "Lord Kazzak") and (.bosses | index("Lord Kazzak")) > (.bosses | index("Ragnaros"))
      and any(.sources[]; .kind == "npc" and .id == 12397 and (.builtin | not) and .name == "Lord Kazzak" and .items == 4)' >/dev/null
call POST "/api/events/$SRE/loot-sources" '{"url":"www.wowhead.com/classic/object=181366"}' | jq -e 'any(.items[]; .item_id == 22809 and .boss == "Four Horsemen Chest")' >/dev/null
fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/npc=12397"}'; grep -q "already" /tmp/body
fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://evil.example/classic/zone=2717"}'
fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/item=19019"}'
fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/npc=4242"}'; grep -q "doesn't have" /tmp/body
fail_call POST "/api/events/$SRE/loot-sources" '{"url":"https://www.wowhead.com/classic/zone=2717"}'; grep -q "own list" /tmp/body
# Every MC raid in the outhouse gets the imported pages.
call GET "/api/events/$SRE/loot-table" | jq -e '[.sources[] | select(.builtin | not)] | length == 2' >/dev/null
call DELETE "/api/events/$SRE/loot-sources?branch=classic&kind=npc&id=12397" | jq -e 'all(.items[]; .item_id != 17111) and ([.sources[] | select(.builtin | not)] | length == 1)' >/dev/null
fail_call DELETE "/api/events/$SRE/loot-sources?branch=classic&kind=npc&id=12397"
call GET "/api/guilds/$GUILD/audit-log?action=loot_source_add" | jq -e '.entries | length == 2 and (.[1].details | contains("Lord Kazzak") and contains("Molten Core"))' >/dev/null
# A WoW: Forever raid that isn't built in: its zone is on Wowhead's Forever branch.
SKY=$(call POST "/api/channels/$LC/events" '{"title":"Spire","starts_at":"'"$START"'","game":"forever","soft_reserves":1,"reserve_lock":"manual","loot_instance":"Skyborne Spire"}' | jq -r .event.id)
call GET "/api/events/$SKY/loot-table" | jq -e '.items == [] and .sources == []' >/dev/null
call POST "/api/events/$SKY/loot-sources" '{"url":"https://www.wowhead.com/forever/zone=99001/skyborne-spire"}' \
  | jq -e '.item_branch == "forever" and .bosses == ["Stormcaller Vael"] and any(.items[]; .item_id == 990003 and .quality == 5 and .slot == "Two-Hand Axe")
      and .sources[0].branch == "forever" and .sources[0].name == "Skyborne Spire"' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/events/$SKY/signup" '{"status":"yes","role":"dps","class":"Warrior","spec":"Fury"}' >/dev/null
call PUT "/api/events/$SKY/reserves" '{"item_id":990003}' | jq -e '.event.reserves[0] | .item_name == "Thunderhead Greataxe" and .quality == 5' >/dev/null
TOKEN=$TOKEN_A
# Item links follow the branch that has the lists; loot log entries say which.
call GET /api/wow/info | jq -e '.wowhead_branches.tbc == "tbc" and .wowhead_branches.era == "classic" and (.wowhead_branches.forever | IN("forever", "classic")) and (.wowhead_base | length > 0)' >/dev/null
call GET "/api/guilds/$GUILD/loot" | jq -e 'all(.entries[]; .branch != "")' >/dev/null
for e in "$NX" "$KZ" "$SKY"; do call DELETE "/api/events/$e" >/dev/null; done

step "loot log (continued)"
# The log outlives the raid.
call DELETE "/api/events/$LP" >/dev/null
call GET "/api/guilds/$GUILD/loot" | jq -e '.entries | length == 1 and .[0].event_id == null and .[0].raid_title == "Ony tonight"' >/dev/null
for e in "$CE" "$SRE" "$DLE"; do call DELETE "/api/events/$e" >/dev/null; done
call DELETE "/api/events/$L1?scope=future" >/dev/null
call DELETE "/api/events/$(pevents "$LSER" | jq -r '.[0].id')?scope=future" >/dev/null

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
KR=$(call POST /api/auth/register '{"username":"kickme","email":"kickme@example.test","password":"password123","birth_date":"1990-01-01"}')
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
# The web client's fingerprint (open pages offer "Refresh to update" when it changes): stable, no sign-in needed.
WB1=$(curl -fsS "$BASE/api/health" | jq -r '.web_build')
WB2=$(curl -fsS "$BASE/api/health" | jq -r '.web_build')
[[ "$WB1" =~ ^[0-9a-f]{16}$ && "$WB1" == "$WB2" ]] || { echo "web_build not a stable fingerprint: $WB1 / $WB2"; exit 1; }
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
  R=$(call POST /api/auth/register "{\"username\":\"invitee$n\",\"email\":\"inviteen@example.test\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\",\"display_name\":\"Invitee $n\"}")
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

step "friends: requests, accept, ignore, cancel, live events"
TOKEN=$TOKEN_A
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-friends-bob.jsonl 25 &
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-friends-bob.jsonl 2>/dev/null && break; sleep 0.25; done
call GET /api/users/@me/relationships | jq -e 'length == 0' >/dev/null
call GET /api/users/@me/privacy | jq -e '.allow_dms == "everyone" and .allow_friend_requests == "everyone"' >/dev/null
call POST /api/users/@me/relationships '{"username":"Bob"}' | jq -e --arg b "$BOB" '.kind == "outgoing" and .user.id == $b' >/dev/null
call POST /api/users/@me/relationships '{"username":"@bob"}' | jq -e '.kind == "outgoing"' >/dev/null # asking twice is fine
fail_call POST /api/users/@me/relationships '{"username":"nobody-by-that-name"}'
fail_call POST /api/users/@me/relationships '{"username":"alice"}' # yourself
for i in $(seq 1 20); do grep -q 'RELATIONSHIP_UPDATE' /tmp/gw-friends-bob.jsonl && break; sleep 0.25; done
grep 'RELATIONSHIP_UPDATE' /tmp/gw-friends-bob.jsonl | jq -e --arg a "$ALICE" 'select(.d.kind == "incoming" and .d.user.id == $a)' >/dev/null
TOKEN=$TOKEN_B
call GET /api/users/@me/relationships | jq -e --arg a "$ALICE" '.[] | select(.user.id == $a) | .kind == "incoming"' >/dev/null
call PUT "/api/users/@me/relationships/$ALICE" | jq -e '.kind == "friend"' >/dev/null
TOKEN=$TOKEN_A
call GET /api/users/@me/relationships | jq -e --arg b "$BOB" '[.[] | select(.user.id == $b and .kind == "friend")] | length == 1' >/dev/null
fail_call POST /api/users/@me/relationships '{"username":"bob"}' # already friends
# Cancel your own request; ignore someone else's.
call PUT "/api/users/@me/relationships/$CARL" | jq -e '.kind == "outgoing"' >/dev/null
call DELETE "/api/users/@me/relationships/$CARL" >/dev/null
TOKEN=$TOKEN_C; call GET /api/users/@me/relationships | jq -e 'length == 0' >/dev/null; TOKEN=$TOKEN_A
call PUT "/api/users/@me/relationships/$CARL" >/dev/null
TOKEN=$TOKEN_C; call DELETE "/api/users/@me/relationships/$ALICE" >/dev/null; TOKEN=$TOKEN_A
call GET /api/users/@me/relationships | jq -e --arg c "$CARL" 'all(.[]; .user.id != $c)' >/dev/null
TOKEN=""; fail_call GET /api/users/@me/relationships; TOKEN=$TOKEN_A

step "friends: privacy settings (friend requests, DMs)"
TOKEN=$TOKEN_C
call PUT /api/users/@me/privacy '{"allow_friend_requests":"nobody"}' | jq -e '.allow_friend_requests == "nobody" and .allow_dms == "everyone"' >/dev/null
fail_call PUT /api/users/@me/privacy '{"allow_dms":"strangers"}'
fail_call PUT /api/users/@me/privacy '{"allow_friend_requests":"maybe"}'
TOKEN=$TOKEN_A; fail_call PUT "/api/users/@me/relationships/$CARL"; TOKEN=$TOKEN_C
call PUT /api/users/@me/privacy '{"allow_friend_requests":"friends_of_friends"}' >/dev/null
TOKEN=$TOKEN_A; fail_call PUT "/api/users/@me/relationships/$CARL" # no friends in common yet
# Carl and Bob become friends (accepting a request isn't limited), so Alice
# (Bob's friend) is now a friend of a friend.
TOKEN=$TOKEN_C; call PUT "/api/users/@me/relationships/$BOB" | jq -e '.kind == "outgoing"' >/dev/null
TOKEN=$TOKEN_B; call PUT "/api/users/@me/relationships/$CARL" | jq -e '.kind == "friend"' >/dev/null
TOKEN=$TOKEN_A; call PUT "/api/users/@me/relationships/$CARL" | jq -e '.kind == "outgoing"' >/dev/null
call DELETE "/api/users/@me/relationships/$CARL" >/dev/null
# People in your outhouses only: the outsider can't, Alice can.
TOKEN=$TOKEN_C; call PUT /api/users/@me/privacy '{"allow_friend_requests":"shared"}' >/dev/null
TOKEN=$OUT; fail_call PUT "/api/users/@me/relationships/$CARL"
TOKEN=$TOKEN_A; call PUT "/api/users/@me/relationships/$CARL" | jq -e '.kind == "outgoing"' >/dev/null
call DELETE "/api/users/@me/relationships/$CARL" >/dev/null
# DMs from friends only: Alice can't start one with Carl (or put him in a group), Bob (a friend) can.
TOKEN=$TOKEN_C; call PUT /api/users/@me/privacy '{"allow_dms":"friends","allow_friend_requests":"everyone"}' | jq -e '.allow_dms == "friends"' >/dev/null
TOKEN=$TOKEN_A
fail_call POST /api/dms '{"user_ids":["'"$CARL"'"]}'
grep -q "only takes DMs from friends" /tmp/body
fail_call POST /api/dms '{"user_ids":["'"$BOB"'","'"$CARL"'"]}'
TOKEN=$TOKEN_B; BCDM=$(call POST /api/dms '{"user_ids":["'"$CARL"'"]}' | jq -r .id)
call POST "/api/channels/$BCDM/messages" '{"content":"friends can DM"}' >/dev/null
TOKEN=$TOKEN_C; call PUT /api/users/@me/privacy '{"allow_dms":"everyone"}' >/dev/null
TOKEN=$TOKEN_A; ACDM=$(call POST /api/dms '{"user_ids":["'"$CARL"'"]}' | jq -r .id)
# Group DM picker: friends come first.
TOKEN=$TOKEN_C; call GET /api/users/@me/contacts | jq -e --arg b "$BOB" '.[0].id == $b' >/dev/null; TOKEN=$TOKEN_A

step "friends: blocking (DMs, requests, presence, pushes)"
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-block-bob.jsonl 20 &
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-block-bob.jsonl 2>/dev/null && break; sleep 0.25; done
TOKEN=$TOKEN_C; call GET "/api/guilds/$GUILD/members" | jq -e --arg b "$BOB" '.[] | select(.id == $b) | .online' >/dev/null; TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/members" | jq -e --arg b "$BOB" '.[] | select(.id == $b) | .online' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/users/@me/blocks/$ALICE" | jq -e --arg a "$ALICE" '.kind == "blocked" and .user.id == $a' >/dev/null
call GET /api/users/@me/relationships | jq -e --arg a "$ALICE" '.[] | select(.user.id == $a) | .kind == "blocked"' >/dev/null
fail_call POST "/api/channels/$DMID/messages" '{"content":"you are blocked"}'
TOKEN=$TOKEN_A
call GET /api/users/@me/relationships | jq -e --arg b "$BOB" 'all(.[]; .user.id != $b)' >/dev/null # the friendship ended
fail_call POST "/api/channels/$DMID/messages" '{"content":"can you hear me?"}'
grep -q "can't message this person" /tmp/body
fail_call PUT "/api/users/@me/relationships/$BOB"
fail_call POST /api/dms '{"user_ids":["'"$BOB"'","'"$CARL"'"]}'
call GET /api/users/@me/contacts | jq -e --arg b "$BOB" 'all(.[]; .id != $b)' >/dev/null
# Bob is online, but not to Alice (Carl still sees him).
call GET "/api/guilds/$GUILD/members" | jq -e --arg b "$BOB" '.[] | select(.id == $b) | .online | not' >/dev/null
call GET /api/dms | jq -e --arg id "$DMID" --arg b "$BOB" '.[] | select(.id == $id) | .recipients[] | select(.id == $b) | .online | not' >/dev/null
TOKEN=$TOKEN_C; call GET "/api/guilds/$GUILD/members" | jq -e --arg b "$BOB" '.[] | select(.id == $b) | .online' >/dev/null; TOKEN=$TOKEN_A
# Messages in shared channels still arrive (the app folds them away).
call POST "/api/channels/$CHAN/messages" '{"content":"alice says hi to everyone"}' >/dev/null
# Unblocking: DMs work again.
TOKEN=$TOKEN_B; call DELETE "/api/users/@me/blocks/$ALICE" >/dev/null
call GET /api/users/@me/relationships | jq -e --arg a "$ALICE" 'all(.[]; .user.id != $a)' >/dev/null; TOKEN=$TOKEN_A
call POST "/api/channels/$DMID/messages" '{"content":"unblocked hello"}' >/dev/null
# Carl (offline, push on) heard about Alice's friend requests.
for i in $(seq 1 20); do
  curl -fsS "$PUSH/received/carl" | jq -e 'any(.[]; .payload.kind == "friend")' >/dev/null 2>&1 && break
  sleep 0.5
done
RC=$(curl -fsS "$PUSH/received/carl")
check_push "$RC" 'any(.[]; .payload.kind == "friend" and .payload.title == "New friend request" and (.payload.body | contains("Alice")) and .payload.url == "/app?friends=1" and .payload.channel_id == "friends")' "carl: friend request push"
check_push "$(curl -fsS "$PUSH/received/bob")" 'all(.[]; (.payload.body // "") | (contains("you are blocked") or contains("can you hear me")) | not)' "bob: nothing from Alice while blocked"

step "voice messages: upload checks (type, size, length, waveform)"
python3 - <<'PY'
import struct
def webm(blocks, duration=None):
    v = bytes([0x1A,0x45,0xDF,0xA3,0x84,0x42,0x86,0x81,0x01])
    v += bytes([0x18,0x53,0x80,0x67,0x01,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF])
    info = bytes([0x2A,0xD7,0xB1,0x83,0x0F,0x42,0x40])
    if duration is not None:
        info += bytes([0x44,0x89,0x88]) + struct.pack(">d", duration)
    v += bytes([0x15,0x49,0xA9,0x66,0x80 | len(info)]) + info
    t = 0
    for c in range(0, blocks, 1000):  # a cluster every 20 s
        v += bytes([0x1F,0x43,0xB6,0x75,0x01,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF])
        v += bytes([0xE7,0x84]) + struct.pack(">I", c * 20)
        for i in range(min(1000, blocks - c)):
            v += bytes([0xA3,0x86,0x81]) + struct.pack(">h", i * 20) + bytes([0x80,0xAA,0xBB])
    return v
open("/tmp/voice.webm","wb").write(webm(150))           # 3 seconds
open("/tmp/voice-long.webm","wb").write(webm(18000))    # 6 minutes
def ogg_page(granule, seq, packet, flags=0):
    segs = [255] * (len(packet) // 255) + [len(packet) % 255]
    return b"OggS" + bytes([0, flags]) + struct.pack("<qIII", granule, 7, seq, 0) + bytes([len(segs)] + segs) + packet
head = b"OpusHead" + bytes([1, 1]) + struct.pack("<HIhB", 312, 48000, 0, 0)
open("/tmp/voice.ogg","wb").write(ogg_page(0, 0, head, 2) + ogg_page(48000 * 4 + 312, 1, b"\x00" * 40, 4))
open("/tmp/big.webm","wb").write(webm(150) + b"\x00" * (10 * 1024 * 1024 + 10))
PY
WAVE=$(printf '\x10\x40\x80\xff\x80\x40\x10\x20' | base64)
vupload() { # vupload CHANNEL FILE META [extra -F...]
  local ch=$1 file=$2 meta=$3; shift 3
  code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN" -F "content=" -F "voice=$meta" -F "files=@$file" "$@" "$BASE/api/channels/$ch/messages/upload")
}
TOKEN=$TOKEN_A
vupload "$CHAN" /tmp/voice.webm '{"duration_ms":2900,"waveform":"'"$WAVE"'"}'
[ "$code" = 200 ] || { echo "voice upload -> $code"; cat /tmp/body; exit 1; }
jq -e --arg w "$WAVE" '.content == "" and (.attachments | length == 1) and .attachments[0].voice and .attachments[0].duration_ms == 3000 and .attachments[0].waveform == $w and .attachments[0].content_type == "audio/webm"' /tmp/body >/dev/null
VURL=$(jq -r '.attachments[0].url' /tmp/body)
[ "$(curl -sS -o /dev/null -w '%{content_type}' "$BASE$VURL")" = "audio/webm" ]
vupload "$CHAN" /tmp/voice.ogg '{"duration_ms":1}'
[ "$code" = 200 ] && jq -e '.attachments[0].voice and .attachments[0].duration_ms == 4000 and .attachments[0].content_type == "audio/ogg" and (.attachments[0] | has("waveform") | not)' /tmp/body >/dev/null
vupload "$CHAN" /tmp/voice-long.webm '{"duration_ms":1000}'
[ "$code" = 400 ] && grep -q "5 minutes" /tmp/body
vupload "$CHAN" /tmp/big.webm '{"duration_ms":1000}'
[ "$code" = 400 ] && grep -q "10 MB" /tmp/body
vupload "$CHAN" /tmp/pixel.png '{"duration_ms":1000}'
[ "$code" = 400 ]
vupload "$CHAN" /tmp/voice.webm '{"duration_ms":1000}' -F "files=@/tmp/voice.ogg"
[ "$code" = 400 ]
vupload "$CHAN" /tmp/voice.webm 'not json'
[ "$code" = 400 ]
cp /tmp/voice.webm /tmp/voice.m4a
vupload "$CHAN" /tmp/voice.m4a '{"duration_ms":999999}'
[ "$code" = 400 ] # a length past 5 minutes, when the file can't tell us
vupload "$CHAN" /tmp/voice.webm '{"duration_ms":2900,"waveform":"!!!"}'
[ "$code" = 200 ] && jq -e '.attachments[0] | has("waveform") | not' /tmp/body >/dev/null # a bad waveform is just dropped
# In a DM and in a thread too; not without Attach Files.
vupload "$DMID" /tmp/voice.webm '{"duration_ms":3000}'
[ "$code" = 200 ]
VT=$(call POST "/api/channels/$CHAN/threads" '{"name":"Voice notes"}' | jq -r .id)
vupload "$CHAN" /tmp/voice.webm '{"duration_ms":3000}' -F "thread_id=$VT"
[ "$code" = 200 ] && jq -e --arg t "$VT" '.thread_id == $t and .attachments[0].voice' /tmp/body >/dev/null
TOKEN=$OUT; vupload "$CHAN" /tmp/voice.webm '{"duration_ms":3000}'; [ "$code" = 404 ]; TOKEN=$TOKEN_A

step "stickers: upload, limits, permissions, sending"
supload() { # supload TOKEN FILE NAME [extra -F...]
  local tok=$1 file=$2 name=$3; shift 3
  code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $tok" -F "name=$name" -F "file=@$file" "$@" "$BASE/api/guilds/$GUILD/stickers")
}
supload "$TOKEN_A" /tmp/pixel.png "Big Pull" -F "emoji=🔥" -F "description=When the tank pulls early"
[ "$code" = 200 ] || { echo "sticker upload -> $code"; cat /tmp/body; exit 1; }
STK=$(jq -r .id /tmp/body)
jq -e '.name == "Big Pull" and .emoji == "🔥" and .description == "When the tank pulls early" and .content_type == "image/png" and (.url | startswith("/stickers/"))' /tmp/body >/dev/null
[ "$(curl -sS -o /dev/null -w '%{content_type}' "$BASE/stickers/$STK")" = "image/png" ]
supload "$TOKEN_B" /tmp/pixel.png "Not allowed"; [ "$code" = 403 ] # needs Manage Outhouse
supload "$TOKEN_A" /tmp/pixel.png "big pull"; [ "$code" = 409 ]   # names are unique (any case)
printf '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>' > /tmp/evil.svg
supload "$TOKEN_A" /tmp/evil.svg "Evil"; [ "$code" = 400 ]
head -c 600000 /dev/urandom > /tmp/big.png; printf '\x89PNG\r\n\x1a\n' | dd of=/tmp/big.png conv=notrunc 2>/dev/null
supload "$TOKEN_A" /tmp/big.png "Too big"; [ "$code" = 400 ]
supload "$TOKEN_A" /tmp/pixel.png "x"; [ "$code" = 400 ]
call GET "/api/guilds/$GUILD/stickers" | jq -e 'length == 1' >/dev/null
TOKEN=$TOKEN_B; call GET /api/users/@me/stickers | jq -e --arg s "$STK" '(.[0].id == "builtin:occupied") and any(.[]; .id == $s) and any(.[]; .id == "builtin:gg")' >/dev/null; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/stickers"; TOKEN=$TOKEN_A
call PATCH "/api/stickers/$STK" '{"name":"Early Pull","emoji":"😬"}' | jq -e '.name == "Early Pull" and .emoji == "😬" and .description == "When the tank pulls early"' >/dev/null
TOKEN=$TOKEN_B; fail_call PATCH "/api/stickers/$STK" '{"name":"Mine now"}'; fail_call DELETE "/api/stickers/$STK"; TOKEN=$TOKEN_A
# Sending: a sticker is a message on its own.
SM=$(call POST "/api/channels/$CHAN/stickers" '{"sticker_id":"'"$STK"'"}')
jq -e --arg s "$STK" '.content == "" and .sticker.id == $s and .sticker.name == "Early Pull" and .sticker.url == "/stickers/\($s)"' <<<"$SM" >/dev/null
SMID=$(jq -r .id <<<"$SM")
TOKEN=$TOKEN_B
call POST "/api/channels/$DMID/stickers" '{"sticker_id":"builtin:gg","reply_to_id":"'"$(call GET "/api/channels/$DMID/messages?limit=1" | jq -r '.[0].id')"'"}' | jq -e '.sticker.id == "builtin:gg" and .sticker.url == "/builtin-stickers/gg.svg" and .reply_to != null' >/dev/null
fail_call POST "/api/channels/$CHAN/stickers" '{"sticker_id":"builtin:nope"}'
fail_call POST "/api/channels/$CHAN/stickers" '{"sticker_id":"12345"}'
TOKEN=$TOKEN_A
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/builtin-stickers/gg.svg")" = 200 ]
call PATCH "/api/channels/$CHAN/messages/$SMID" '{"content":"with a caption"}' | jq -e '.sticker != null' >/dev/null
# Up to 60 per outhouse.
for i in $(seq 2 60); do supload "$TOKEN_A" /tmp/pixel.png "Sticker $i"; [ "$code" = 200 ] || { echo "sticker $i -> $code"; cat /tmp/body; exit 1; }; done
supload "$TOKEN_A" /tmp/pixel.png "Sticker 61"; [ "$code" = 400 ] && grep -q "60 stickers" /tmp/body
# Deleting: the picture goes, old messages keep the sticker's name.
call DELETE "/api/stickers/$STK" >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/stickers/$STK")" = 404 ]
call GET "/api/channels/$CHAN/messages?around=$SMID&limit=3" | jq -e --arg m "$SMID" '.[] | select(.id == $m) | .sticker.name == "Early Pull"' >/dev/null
fail_call POST "/api/channels/$CHAN/stickers" '{"sticker_id":"'"$STK"'"}'
call GET "/api/guilds/$GUILD/audit-log?action=sticker_create" | jq -e '.entries | length >= 50' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=sticker_delete" | jq -e '.entries[0].target_name == "Early Pull"' >/dev/null

step "forwarding: snapshot, shared files, 18+ rule, permissions"
FC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"loot"}' | jq -r .id)
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "content=check this drop <:nope:1>" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$FC/messages/upload")
[ "$code" = 200 ]; ORIG=$(jq -r .id /tmp/body); OURL=$(jq -r '.attachments[0].url' /tmp/body)
FW=$(call POST "/api/channels/$FC/messages/$ORIG/forward" '{"destinations":["'"$DMID"'","'"$ACDM"'"],"note":"look at this"}')
jq -e --arg o "$ORIG" --arg u "$OURL" --arg a "$ALICE" 'length == 2 and all(.[]; .content == "look at this" and .forward.message_id == $o and .forward.channel_name == "#loot" and .forward.guild_name == "Smoke Test" and .forward.author_id == $a and (.forward.content | startswith("check this drop")) and .attachments[0].url == $u)' <<<"$FW" >/dev/null
FW1=$(jq -r '.[0].id' <<<"$FW"); FW2=$(jq -r '.[1].id' <<<"$FW")
FWDM=$(jq -r '.[0].channel_id' <<<"$FW")
# Bob sees it in the DM, and can open the file.
TOKEN=$TOKEN_B
call GET "/api/channels/$DMID/messages?limit=5" | jq -e --arg f "$FW1" 'any(.[]; .id == $f and .forward.channel_name == "#loot")' >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$OURL")" = 200 ]
# A forward of a forward passes on the original.
FF=$(call POST "/api/channels/$FWDM/messages/$FW1/forward" '{"destinations":["'"$CHAN"'"]}')
jq -e --arg o "$ORIG" '.[0].forward.message_id == $o and .[0].forward.channel_name == "#loot" and .[0].content == ""' <<<"$FF" >/dev/null
FF1=$(jq -r '.[0].id' <<<"$FF")
TOKEN=$TOKEN_A
# The file lives as long as any message uses it.
call DELETE "/api/channels/$FC/messages/$ORIG" >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$OURL")" = 200 ]
call DELETE "/api/channels/$DMID/messages/$FW1" >/dev/null
call DELETE "/api/channels/$ACDM/messages/$FW2" >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$OURL")" = 200 ]
TOKEN=$TOKEN_B; call DELETE "/api/channels/$CHAN/messages/$FF1" >/dev/null; TOKEN=$TOKEN_A
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$OURL")" = 404 ]
# Limits and permissions.
FM=$(call POST "/api/channels/$FC/messages" '{"content":"forward me"}' | jq -r .id)
fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":[]}'
fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":["1","2","3","4","5","6"]}'
fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":["'"$CHAN"'","999"]}'
call GET "/api/channels/$CHAN/messages?limit=3" | jq -e 'all(.[]; .content != "forward me" and .forward.content != "forward me")' >/dev/null # nothing half-sent
fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":["'"$CHAN"'"],"note":"'"$(head -c 2001 /dev/zero | tr '\0' a)"'"}'
TOKEN=$OUT; fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":["'"$CHAN"'"]}'; TOKEN=$TOKEN_A
# Only into places you can post in: Bob can't post in a read-only channel.
RO=$(call POST "/api/guilds/$GUILD/channels" '{"name":"announcements"}' | jq -r .id)
call PUT "/api/channels/$RO/permissions/$GUILD" '{"kind":"role","allow":"0","deny":"2"}' >/dev/null
TOKEN=$TOKEN_B; fail_call POST "/api/channels/$FC/messages/$FM/forward" '{"destinations":["'"$RO"'"]}'; TOKEN=$TOKEN_A
# Age-restricted (18+): only to other 18+ channels; you have to have said you're 18+ to see it.
N1=$(call POST "/api/guilds/$GUILD/channels" '{"name":"after-hours"}' | jq -r .id)
N2=$(call POST "/api/guilds/$GUILD/channels" '{"name":"after-hours-2"}' | jq -r .id)
call PATCH "/api/channels/$N1" '{"content_mode":"nsfw"}' >/dev/null
call PATCH "/api/channels/$N2" '{"content_mode":"nsfw"}' >/dev/null
call PUT "/api/channels/$N1/age-gate" >/dev/null; call PUT "/api/channels/$N2/age-gate" >/dev/null
NM1=$(call POST "/api/channels/$N1/messages" '{"content":"grown-up joke"}' | jq -r .id)
fail_call POST "/api/channels/$N1/messages/$NM1/forward" '{"destinations":["'"$CHAN"'"]}'
grep -q "age-restricted" /tmp/body
fail_call POST "/api/channels/$N1/messages/$NM1/forward" '{"destinations":["'"$DMID"'"]}'
fail_call POST "/api/channels/$N1/messages/$NM1/forward" '{"destinations":["'"$N2"'","'"$CHAN"'"]}'
call POST "/api/channels/$N1/messages/$NM1/forward" '{"destinations":["'"$N2"'"]}' | jq -e '.[0].forward.content == "grown-up joke"' >/dev/null
TOKEN=$TOKEN_B; fail_call POST "/api/channels/$N1/messages/$NM1/forward" '{"destinations":["'"$N1"'"]}'; jq -e '.code == "age_restricted"' /tmp/body >/dev/null; TOKEN=$TOKEN_A
# A raid card or poll on its own can't be forwarded.
PM=$(call POST "/api/channels/$FC/polls" '{"question":"Ony tonight?","options":["yes","no"]}' | jq -r .id)
fail_call POST "/api/channels/$FC/messages/$PM/forward" '{"destinations":["'"$CHAN"'"]}'

step "scheduled messages: create, edit, cancel, send now, limits"
SC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"later"}' | jq -r .id)
SOON=$(date -u -d '+25 seconds' +%Y-%m-%dT%H:%M:%SZ)
LATER=$(date -u -d '+2 days' +%Y-%m-%dT09:00:00Z)
S1=$(call POST "/api/channels/$SC/scheduled" '{"content":"good morning raiders","send_at":"'"$SOON"'"}')
jq -e --arg c "$SC" --arg g "$GUILD" '.status == "pending" and .channel_id == $c and .guild_id == $g and .content == "good morning raiders"' <<<"$S1" >/dev/null
S1ID=$(jq -r .id <<<"$S1")
S2ID=$(call POST "/api/channels/$SC/scheduled" '{"content":"reminder: flasks","send_at":"'"$LATER"'"}' | jq -r .id)
S3ID=$(call POST "/api/channels/$SC/scheduled" '{"content":"cancel me","send_at":"'"$LATER"'"}' | jq -r .id)
fail_call POST "/api/channels/$SC/scheduled" '{"content":"in the past","send_at":"2020-01-01T00:00:00Z"}'
fail_call POST "/api/channels/$SC/scheduled" '{"content":"too far","send_at":"2099-01-01T00:00:00Z"}'
fail_call POST "/api/channels/$SC/scheduled" '{"content":"   ","send_at":"'"$LATER"'"}'
TOKEN=$OUT; fail_call POST "/api/channels/$SC/scheduled" '{"content":"not mine","send_at":"'"$LATER"'"}'; TOKEN=$TOKEN_A
call GET /api/users/@me/scheduled | jq -e --arg a "$S1ID" --arg b "$S2ID" 'length == 3 and .[0].id == $a and .[1].id == $b' >/dev/null
call PATCH "/api/scheduled/$S2ID" '{"content":"reminder: flasks and food"}' | jq -e '.content == "reminder: flasks and food" and .status == "pending"' >/dev/null
fail_call PATCH "/api/scheduled/$S2ID" '{"send_at":"2020-01-01T00:00:00Z"}'
TOKEN=$TOKEN_B; fail_call PATCH "/api/scheduled/$S2ID" '{"content":"hijack"}'; fail_call DELETE "/api/scheduled/$S3ID"; fail_call POST "/api/scheduled/$S2ID/send"; TOKEN=$TOKEN_A
call DELETE "/api/scheduled/$S3ID" >/dev/null
fail_call DELETE "/api/scheduled/$S3ID"
call POST "/api/scheduled/$S2ID/send" | jq -e --arg c "$SC" '.content == "reminder: flasks and food" and .channel_id == $c' >/dev/null
call GET /api/users/@me/scheduled | jq -e --arg a "$S1ID" 'length == 1 and .[0].id == $a' >/dev/null
# Up to 50 waiting per person.
TOKEN=$TOKEN_C
for i in $(seq 1 50); do call POST "/api/channels/$CHAN/scheduled" '{"content":"note '"$i"'","send_at":"'"$LATER"'"}' >/dev/null; done
fail_call POST "/api/channels/$CHAN/scheduled" '{"content":"one too many","send_at":"'"$LATER"'"}'
grep -q "50 messages" /tmp/body
for id in $(call GET /api/users/@me/scheduled | jq -r '.[].id'); do call DELETE "/api/scheduled/$id" >/dev/null; done
call GET /api/users/@me/scheduled | jq -e 'length == 0' >/dev/null
# Permissions are checked again when it's due: Bob's goes nowhere once he's timed out,
# Carl's once the channel is deleted.
SOON2=$(date -u -d '+12 seconds' +%Y-%m-%dT%H:%M:%SZ)
TOKEN=$TOKEN_B; SB=$(call POST "/api/channels/$SC/scheduled" '{"content":"bob was here","send_at":"'"$SOON2"'"}' | jq -r .id)
GONE=$(TOKEN=$TOKEN_A call POST "/api/guilds/$GUILD/channels" '{"name":"doomed"}' | jq -r .id)
TOKEN=$TOKEN_C; SCARL=$(call POST "/api/channels/$GONE/scheduled" '{"content":"into the void","send_at":"'"$SOON2"'"}' | jq -r .id)
TOKEN=$TOKEN_A
call POST "/api/guilds/$GUILD/members/$BOB/timeout" '{"minutes":60}' >/dev/null
call DELETE "/api/channels/$GONE" >/dev/null

step "scheduled messages: the background job sends them (or says why not)"
for i in $(seq 1 60); do
  call GET "/api/channels/$SC/messages?limit=10" | jq -e 'any(.[]; .content == "good morning raiders")' >/dev/null 2>&1 \
    && TOKEN=$TOKEN_B call GET /api/users/@me/scheduled | jq -e '.[0].status == "failed"' >/dev/null 2>&1 \
    && TOKEN=$TOKEN_C call GET /api/users/@me/scheduled | jq -e '.[0].status == "failed"' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$SC/messages?limit=10" | jq -e --arg a "$ALICE" '[.[] | select(.content == "good morning raiders" and .author_id == $a)] | length == 1' >/dev/null || { echo "scheduled message never sent"; exit 1; }
call GET /api/users/@me/scheduled | jq -e 'length == 0' >/dev/null
call GET "/api/channels/$SC/messages?limit=10" | jq -e 'all(.[]; .content != "bob was here")' >/dev/null
TOKEN=$TOKEN_B
call GET /api/users/@me/scheduled | jq -e --arg s "$SB" '.[0].id == $s and .[0].status == "failed" and (.[0].error | contains("can'"'"'t post there"))' >/dev/null || { call GET /api/users/@me/scheduled; exit 1; }
TOKEN=$TOKEN_C
call GET /api/users/@me/scheduled | jq -e --arg s "$SCARL" '.[0].id == $s and .[0].status == "failed" and (.[0].error | contains("gone"))' >/dev/null || { call GET /api/users/@me/scheduled; exit 1; }
call DELETE "/api/scheduled/$SCARL" >/dev/null
TOKEN=$TOKEN_A; call POST "/api/guilds/$GUILD/members/$BOB/timeout" '{"minutes":0}' >/dev/null
# Once he can post again, Bob tries again with a new time, or sends it now.
TOKEN=$TOKEN_B
call PATCH "/api/scheduled/$SB" '{"send_at":"'"$LATER"'"}' | jq -e '.status == "pending" and .error == null' >/dev/null
call POST "/api/scheduled/$SB/send" | jq -e '.content == "bob was here"' >/dev/null
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
call POST /api/auth/register '{"username":"lockme","email":"lockme@example.test","password":"password123","birth_date":"1990-01-01"}' >/dev/null
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
    -d '{"username":"!","email":"someone@example.test","password":"password123","birth_date":"1990-01-01"}' "$BASE/api/auth/register")
  [ "$code" = 400 ]
done
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: 203.0.113.60" \
  -d '{"username":"toomany","email":"toomany@example.test","password":"password123","birth_date":"1990-01-01"}' "$BASE/api/auth/register")
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
  [ "$(reg '{"username":"reg'"$mode"'1","email":"reg'"$mode"'1@example.test","password":"password123","birth_date":"1990-01-01"}')" = 403 ] || { echo "$mode: signed up without an invite"; cat /tmp/body; exit 1; }
  [ "$(reg '{"username":"reg'"$mode"'2","email":"reg'"$mode"'2@example.test","password":"password123","birth_date":"1990-01-01","invite":"nope1234"}')" = 403 ]
  if [ "$mode" = invite ]; then
    grep -q "need an invite" /tmp/body
    [ "$(reg '{"username":"reginvite3","email":"reginvite3@example.test","password":"password123","birth_date":"1990-01-01","invite":"'"$REG_INV"'"}')" = 200 ] || { cat /tmp/body; exit 1; }
    T3=$(jq -r .token /tmp/body)
    curl -fsS -X POST -H "Authorization: Bearer $T3" "$R/api/invites/$REG_INV" | jq -e --arg g "$GUILD" '.id == $g' >/dev/null
  else
    grep -q "closed" /tmp/body
    [ "$(reg '{"username":"regclosed3","email":"regclosed3@example.test","password":"password123","birth_date":"1990-01-01","invite":"'"$REG_INV"'"}')" = 403 ]
  fi
  # Signing in still works.
  curl -fsS -H "Content-Type: application/json" -d '{"username":"bob","password":"password123"}' "$R/api/auth/login" >/dev/null
  kill $REG_PID; wait $REG_PID 2>/dev/null || true
done
TOKEN=$TOKEN_A

# ---------------------------------------------------------------------------
# Bot message components, automod and thread auto-archive (feat-botsmod)
# ---------------------------------------------------------------------------
# status METHOD PATH [JSON] -> prints just the HTTP status (body in /tmp/body)
status() {
  local method=$1 path=$2 body=${3:-}
  local args=(-sS -X "$method" -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$TOKEN" ] && args+=(-H "Authorization: Bearer $TOKEN")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
# wait_for FILE JQ_FILTER [ARGS...] -> waits up to 10 s for a gateway event matching the filter
wait_for() {
  local file=$1 filter=$2; shift 2
  for _ in $(seq 1 40); do
    jq -e -s "$@" "[.[] | select($filter)] | length > 0" "$file" >/dev/null 2>&1 && return 0
    sleep 0.25
  done
  echo "never saw $filter in $file:"; tail -5 "$file"; return 1
}

step "components: bots can add buttons and select menus (Discord's shape)"
TOKEN=$TOKEN_A
CB=$(call POST "/api/guilds/$GUILD/bots" '{"name":"Buttons"}'); CB_TOKEN=$(jq -r .token <<<"$CB"); CB_ID=$(jq -r .bot.id <<<"$CB")
CC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"bot-buttons"}' | jq -r .id)
ROWS='[{"type":1,"components":[{"type":2,"style":1,"label":"Again","emoji":{"name":"🎲"},"custom_id":"again"},{"type":2,"style":4,"label":"Off","custom_id":"off","disabled":true},{"type":2,"style":5,"label":"Docs","url":"https://example.com/docs"}]},{"type":1,"components":[{"type":3,"custom_id":"size","placeholder":"Pick","min_values":1,"max_values":2,"options":[{"label":"d6","value":"6"},{"label":"d20","value":"20","description":"classic"},{"label":"d100","value":"100"}]}]}]'
TOKEN=$CB_TOKEN
CM=$(call POST "/api/channels/$CC/messages" "$(jq -nc --argjson c "$ROWS" '{content:"Pick one",components:$c}')")
jq -e '(.components | length) == 2 and .components[0].components[2].url == "https://example.com/docs"
       and .components[1].components[0].max_values == 2 and .components[0].components[1].disabled' <<<"$CM" >/dev/null
CMID=$(jq -r .id <<<"$CM")
BAD_ROWS=(
  '[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"javascript:alert(1)"}]}]'
  '[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":" JavaScript:alert(1)"}]}]'
  '[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"data:text/html,hi"}]}]'
  '[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"/api/users/@me"}]}]'
  '[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"https://ok.example","custom_id":"y"}]}]'
  '[{"type":1,"components":[{"type":2,"style":1,"label":"x","url":"https://ok.example","custom_id":"y"}]}]'
  '[{"type":1,"components":[{"type":2,"style":1,"label":"no id"}]}]'
  '[{"type":1,"components":[{"type":2,"style":1,"custom_id":"no-label"}]}]'
  '[{"type":1,"components":[{"type":2,"style":9,"label":"x","custom_id":"x"}]}]'
  '[{"type":1,"components":[{"type":2,"style":1,"label":"a","custom_id":"x"},{"type":2,"style":1,"label":"b","custom_id":"x"}]}]'
  '[{"type":2,"style":1,"label":"a","custom_id":"x"}]'
  '[{"type":1,"components":[]}]'
  '[{"type":1,"components":[{"type":3,"custom_id":"s","options":[{"label":"a","value":"a"}]},{"type":2,"style":1,"label":"b","custom_id":"b"}]}]'
  '[{"type":1,"components":[{"type":3,"custom_id":"s","options":[]}]}]'
  '[{"type":1,"components":[{"type":3,"custom_id":"s","max_values":2,"options":[{"label":"a","value":"a"}]}]}]'
  '[{"type":1,"components":[{"type":3,"custom_id":"s","options":[{"label":"a","value":"a"},{"label":"b","value":"a"}]}]}]'
)
BAD_ROWS+=("$(jq -nc '[{type:1,components:[{type:2,style:1,label:"x",custom_id:("x" * 101)}]}]')")
BAD_ROWS+=("$(jq -nc '[range(6) | {type:1,components:[{type:2,style:1,label:"x",custom_id:("r\(.)")}]}]')")
BAD_ROWS+=("$(jq -nc '[{type:1,components:[range(6) | {type:2,style:1,label:"x",custom_id:("b\(.)")}]}]')")
BAD_ROWS+=("$(jq -nc '[{type:1,components:[{type:3,custom_id:"s",options:[range(26) | {label:"o\(.)",value:"v\(.)"}]}]}]')")
for rows in "${BAD_ROWS[@]}"; do
  fail_call POST "/api/channels/$CC/messages" "$(jq -nc --argjson c "$rows" '{content:"x",components:$c}')"
done
# Nothing bad got stored, and people (not bots) can't add components at all.
TOKEN=$TOKEN_B
fail_call POST "/api/channels/$CC/messages" "$(jq -nc --argjson c "$ROWS" '{content:"x",components:$c}')"
call GET "/api/channels/$CC/messages" | jq -e 'length == 1 and .[0].components[0].components[0].custom_id == "again"' >/dev/null
# A bot can change its buttons later (PATCH), keeping the text.
TOKEN=$CB_TOKEN
call PATCH "/api/channels/$CC/messages/$CMID" "$(jq -nc --argjson c "$ROWS" '{components:$c}')" | jq -e '.content == "Pick one" and (.components | length) == 2 and .edited_at == null' >/dev/null
fail_call PATCH "/api/channels/$CC/messages/$CMID" '{"components":[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"javascript:void(0)"}]}]}'

step "components: clicks reach the bot over the gateway"
node ci/gateway_probe.mjs "$BASE" "$CB_TOKEN" /tmp/gw-cb.jsonl 90 & CB_PROBE=$!
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-cb-bob.jsonl 90 & CBB_PROBE=$!
node ci/gateway_probe.mjs "$BASE" "$TOKEN_A" /tmp/gw-cb-alice.jsonl 90 & CBA_PROBE=$!
for f in /tmp/gw-cb.jsonl /tmp/gw-cb-bob.jsonl /tmp/gw-cb-alice.jsonl; do wait_for "$f" '.t == "READY"'; done
TOKEN=$TOKEN_B
I1=$(call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}' | jq -r .id)
wait_for /tmp/gw-cb.jsonl '.t == "INTERACTION_CREATE" and .d.id == $i' --arg i "$I1"
jq -e -s --arg i "$I1" --arg m "$CMID" --arg c "$CC" --arg g "$GUILD" --arg b "$BOB" '
  [.[] | select(.t == "INTERACTION_CREATE" and .d.id == $i)][0].d
  | .type == 3 and .data.custom_id == "again" and .data.component_type == 2 and .message.id == $m
    and .channel_id == $c and .guild_id == $g and .user.id == $b' /tmp/gw-cb.jsonl >/dev/null
# Only real components on that message, switched on, with real choices
# (Alice tries these, so Bob stays well under the click limit).
PM=$(call POST "/api/channels/$CC/messages" '{"content":"just me"}' | jq -r .id)
TOKEN=$TOKEN_A
fail_call POST "/api/channels/$CC/messages/$PM/interactions" '{"custom_id":"again"}'
fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"nope"}'
fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"off"}'
fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"size","values":["7"]}'
fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"size","values":[]}'
fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"size","values":["6","20","100"]}'
TOKEN=$TOKEN_B
I2=$(call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"size","values":["6","20"]}' | jq -r .id)
wait_for /tmp/gw-cb.jsonl '.t == "INTERACTION_CREATE" and .d.id == $i and .d.data.values == ["6","20"] and .d.data.component_type == 3' --arg i "$I2"
# Not from outside the outhouse, and not by bots.
TOKEN=$OUT; fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}'
TOKEN=$CB_TOKEN; fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}'

step "components: the bot answers (defer, ephemeral reply, reply, update)"
TOKEN=$TOKEN_B; fail_call POST "/api/interactions/$I1/callback" '{"type":6}'   # not Bob's to answer
TOKEN=$CB_TOKEN
fail_call POST "/api/interactions/$I1/callback" '{"type":9}'
[ "$(status POST "/api/interactions/$I1/callback" '{"type":6}')" = 204 ]
wait_for /tmp/gw-cb-bob.jsonl '.t == "INTERACTION_SUCCESS" and .d.id == $i' --arg i "$I1"
# Ephemeral: only Bob gets it, and it's never saved.
E=$(call POST "/api/interactions/$I2/callback" '{"type":4,"data":{"content":"Only Bob sees this","flags":64,"components":[{"type":1,"components":[{"type":2,"style":3,"label":"Yes","custom_id":"eph-yes"}]}]}}')
jq -e '.ephemeral == true and .content == "Only Bob sees this" and .reply_to == null' <<<"$E" >/dev/null
EID=$(jq -r .id <<<"$E")
wait_for /tmp/gw-cb-bob.jsonl '.t == "MESSAGE_CREATE" and .d.id == $e and .d.ephemeral' --arg e "$EID"
wait_for /tmp/gw-cb-bob.jsonl '.t == "INTERACTION_SUCCESS" and .d.id == $i' --arg i "$I2"
TOKEN=$TOKEN_A
call GET "/api/channels/$CC/messages" | jq -e --arg e "$EID" 'all(.[]; .id != $e)' >/dev/null
# Its button works for Bob only, and the bot can update it.
fail_call POST "/api/channels/$CC/messages/$EID/interactions" '{"custom_id":"eph-yes"}'
TOKEN=$TOKEN_B
I3=$(call POST "/api/channels/$CC/messages/$EID/interactions" '{"custom_id":"eph-yes"}' | jq -r .id)
wait_for /tmp/gw-cb.jsonl '.t == "INTERACTION_CREATE" and .d.id == $i and .d.message.ephemeral' --arg i "$I3"
TOKEN=$CB_TOKEN
call POST "/api/interactions/$I3/callback" '{"type":7,"data":{"content":"Done!","components":[]}}' | jq -e '.ephemeral and .content == "Done!"' >/dev/null
wait_for /tmp/gw-cb-bob.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $e and .d.content == "Done!"' --arg e "$EID"
# A public reply to a click replies to the message with the button.
TOKEN=$TOKEN_B
I4=$(call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}' | jq -r .id)
TOKEN=$CB_TOKEN
[ "$(status POST "/api/interactions/$I4/callback" '{"type":5}')" = 204 ]
call POST "/api/interactions/$I4/callback" '{"type":4,"data":{"content":"Bob rolled 17"}}' | jq -e --arg m "$CMID" '.reply_to.id == $m and .author_bot and (.ephemeral | not)' >/dev/null
wait_for /tmp/gw-cb-alice.jsonl '.t == "MESSAGE_CREATE" and .d.content == "Bob rolled 17"'
# Update the message itself (type 7): new text, buttons gone.
TOKEN=$TOKEN_B
I5=$(call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}' | jq -r .id)
TOKEN=$CB_TOKEN
fail_call POST "/api/interactions/$I5/callback" '{"type":7,"data":{"components":[{"type":1,"components":[{"type":2,"style":5,"label":"x","url":"javascript:1"}]}]}}'
call POST "/api/interactions/$I5/callback" '{"type":7,"data":{"content":"All rolled","components":[]}}' | jq -e '.content == "All rolled" and (.components // [] | length) == 0' >/dev/null
wait_for /tmp/gw-cb-alice.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $m and .d.content == "All rolled"' --arg m "$CMID"
TOKEN=$TOKEN_B; fail_call POST "/api/channels/$CC/messages/$CMID/interactions" '{"custom_id":"again"}'
# Alice never saw Bob's ephemeral message.
if jq -e -s --arg e "$EID" '[.[] | select(.d.id? == $e)] | length > 0' /tmp/gw-cb-alice.jsonl >/dev/null; then echo "ephemeral message leaked to Alice"; exit 1; fi

step "components: rate limit and offline bots"
TOKEN=$TOKEN_A
CM2=$(TOKEN=$CB_TOKEN call POST "/api/channels/$CC/messages" "$(jq -nc --argjson c "$ROWS" '{content:"Again",components:$c}')" | jq -r .id)
TOKEN=$TOKEN_C
for i in $(seq 1 10); do s=$(status POST "/api/channels/$CC/messages/$CM2/interactions" '{"custom_id":"nope"}'); [ "$s" = 400 ] || { echo "click $i -> $s"; cat /tmp/body; exit 1; }; done
[ "$(status POST "/api/channels/$CC/messages/$CM2/interactions" '{"custom_id":"again"}')" = 429 ] || { echo "no rate limit on clicks"; cat /tmp/body; exit 1; }
kill $CB_PROBE 2>/dev/null || true; wait $CB_PROBE 2>/dev/null || true
sleep 1
TOKEN=$TOKEN_B
[ "$(status POST "/api/channels/$CC/messages/$CM2/interactions" '{"custom_id":"again"}')" = 400 ] && grep -q "isn't running" /tmp/body

step "components: the example dice bot (Roll again, Roll in secret, dice size)"
TOKEN=$TOKEN_A
DB=$(call POST "/api/guilds/$GUILD/bots" '{"name":"Dicey"}'); DB_TOKEN=$(jq -r .token <<<"$DB"); DB_ID=$(jq -r .bot.id <<<"$DB")
OUTHOUSE_URL=$BASE OUTHOUSE_BOT_TOKEN=$DB_TOKEN node web/bots/dice-bot.mjs >/tmp/bot2.log 2>&1 &
DB_PID=$!
for i in $(seq 1 20); do call GET "/api/guilds/$GUILD/commands" | jq -e 'any(.[]; .name == "roll")' >/dev/null 2>&1 && break; sleep 1; done
ROLL2=$(call GET "/api/guilds/$GUILD/commands" | jq -r '.[] | select(.name == "roll") | .id')
TOKEN=$TOKEN_B
call POST "/api/channels/$CC/interactions" '{"command_id":"'"$ROLL2"'","options":{"dice":"d6"}}' >/dev/null
for i in $(seq 1 20); do call GET "/api/channels/$CC/messages" | jq -e '.[-1].interaction.command == "roll"' >/dev/null 2>&1 && break; sleep 0.5; done
DM=$(call GET "/api/channels/$CC/messages" | jq -c '.[-1]')
jq -e '.interaction.command == "roll" and .components[0].components[0].label == "Roll again" and .components[1].components[0].custom_id == "size"' <<<"$DM" >/dev/null || { echo "dice bot reply has no buttons: $DM"; cat /tmp/bot2.log; exit 1; }
DMID=$(jq -r .id <<<"$DM"); AGAIN=$(jq -r '.components[0].components[0].custom_id' <<<"$DM"); SECRET=$(jq -r '.components[0].components[1].custom_id' <<<"$DM")
call POST "/api/channels/$CC/messages/$DMID/interactions" "{\"custom_id\":\"$AGAIN\"}" >/dev/null
wait_for /tmp/gw-cb-bob.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $m and (.d.content | test("1-6"))' --arg m "$DMID"
call POST "/api/channels/$CC/messages/$DMID/interactions" "{\"custom_id\":\"$SECRET\"}" >/dev/null
wait_for /tmp/gw-cb-bob.jsonl '.t == "MESSAGE_CREATE" and .d.ephemeral and (.d.content | test("Just between us"))'
call POST "/api/channels/$CC/messages/$DMID/interactions" '{"custom_id":"size","values":["20"]}' >/dev/null
wait_for /tmp/gw-cb-bob.jsonl '.t == "MESSAGE_CREATE" and (.d.ephemeral | not) and (.d.content | test("1-20")) and .d.reply_to.id == $m' --arg m "$DMID"
kill $DB_PID $CBB_PROBE $CBA_PROBE 2>/dev/null || true
TOKEN=$TOKEN_A
call DELETE "/api/bots/$DB_ID" >/dev/null

step "automod: settings, permissions and the outhouse-wide list"
TOKEN=$TOKEN_A
AM=$(call POST "/api/guilds/$GUILD/channels" '{"name":"automod-test"}' | jq -r .id)
AML=$(call POST "/api/guilds/$GUILD/channels" '{"name":"mod-log"}' | jq -r .id)
call GET "/api/channels/$AM/automod" | jq -e '(.word_filter | not) and (.spam_filter | not) and .exempt_moderators and .use_default_words and .whole_word' >/dev/null
DORA=$(TOKEN="" call POST /api/auth/register '{"username":"dora","email":"dora@example.test","password":"password123","birth_date":"1990-01-01","display_name":"Dora"}')
TOKEN_D=$(jq -r .token <<<"$DORA"); DORA_ID=$(jq -r .user.id <<<"$DORA")
AMINV=$(call POST "/api/guilds/$GUILD/invites" | jq -r .code)
TOKEN=$TOKEN_D; call POST "/api/invites/$AMINV" >/dev/null
fail_call GET "/api/channels/$AM/automod"
fail_call PUT "/api/channels/$AM/automod" '{"word_filter":true}'
fail_call PUT "/api/guilds/$GUILD/automod" '{"words":["x"]}'
TOKEN=$TOKEN_A
call PUT "/api/guilds/$GUILD/automod" '{"words":["Murloc*","  ","murloc*"]}' | jq -e '.words == ["murloc*"]' >/dev/null
call GET "/api/guilds/$GUILD/automod" | jq -e '.words == ["murloc*"]' >/dev/null
fail_call PUT "/api/channels/$AM/automod" '{"alert_channel_id":"123"}'
fail_call PUT "/api/channels/$AM/automod" "$(jq -nc '{words:[("x" * 61)]}')"
call PUT "/api/channels/$AM/automod" '{"word_filter":true,"words":["dang it","N00B"," "],"leetspeak":true,"alert_channel_id":"'"$AML"'","exempt_roles":["'"$ROLE"'","42"]}' \
  | jq -e --arg r "$ROLE" '.words == ["dang it","n00b"] and .exempt_roles == [$r] and .word_action == "block"' >/dev/null

step "automod: word filter blocks, with leetspeak, wildcards and the outhouse list"
TOKEN=$TOKEN_D
blocked() { # blocked JSON [path] -> the message was refused by automod
  local s; s=$(status POST "${2:-/api/channels/$AM/messages}" "$1")
  [ "$s" = 403 ] && grep -q "Your message was blocked by automod" /tmp/body || { echo "not blocked ($s): $1"; cat /tmp/body; exit 1; }
}
blocked '{"content":"well DANG   it all"}'
blocked '{"content":"you n00b"}'
blocked '{"content":"you noob"}'
blocked '{"content":"a murlocs raid"}'
blocked '{"content":"MurlocLord here"}'
call POST "/api/channels/$AM/messages" '{"content":"a clean message"}' >/dev/null
call POST "/api/channels/$AM/messages" '{"content":"noobish but whole words only"}' >/dev/null
# Edits, thread replies and polls are checked too.
OK=$(call POST "/api/channels/$AM/messages" '{"content":"fine for now"}' | jq -r .id)
[ "$(status PATCH "/api/channels/$AM/messages/$OK" '{"content":"dang it"}')" = 403 ]
call GET "/api/channels/$AM/messages" | jq -e --arg m "$OK" '.[] | select(.id == $m) | .content == "fine for now"' >/dev/null
AT=$(TOKEN=$TOKEN_A call POST "/api/channels/$AM/threads" '{"name":"Side chat"}' | jq -r .id)
blocked '{"content":"dang it","thread_id":"'"$AT"'"}'
blocked '{"question":"dang it?","options":["yes","no"]}' "/api/channels/$AM/polls"
# The outhouse list can be switched off per channel.
TOKEN=$TOKEN_A
call PUT "/api/channels/$AM/automod" '{"word_filter":true,"words":["dang it","n00b"],"leetspeak":true,"use_default_words":false,"alert_channel_id":"'"$AML"'","exempt_roles":["'"$ROLE"'"]}' >/dev/null
TOKEN=$TOKEN_D; call POST "/api/channels/$AM/messages" '{"content":"a murlocs raid"}' >/dev/null
# Moderators and exempt roles (Bob is a Raider) skip it.
TOKEN=$TOKEN_A; call POST "/api/channels/$AM/messages" '{"content":"dang it, says the boss"}' >/dev/null
TOKEN=$TOKEN_B; call POST "/api/channels/$AM/messages" '{"content":"dang it, says a raider"}' >/dev/null
# Bots skip it unless "check bots" is on.
TOKEN=$CB_TOKEN; call POST "/api/channels/$AM/messages" '{"content":"dang it, says a bot"}' >/dev/null
TOKEN=$TOKEN_A
call PUT "/api/channels/$AM/automod" '{"word_filter":true,"words":["dang it"],"check_bots":true,"exempt_moderators":false,"alert_channel_id":"'"$AML"'"}' >/dev/null
TOKEN=$CB_TOKEN; blocked '{"content":"dang it, says a bot again"}'
TOKEN=$TOKEN_A; blocked '{"content":"dang it, the boss again"}'
# The audit log has the rule and a short snippet only; the mod-log channel gets an alert.
TOKEN=$TOKEN_D; blocked "$(jq -nc '{content:("dang it " + ("blah " * 60))}')"
TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/audit-log?action=automod_block" | jq -e --arg d "$DORA_ID" '
  (.entries | length) >= 8 and (.entries[0].actor_id == $d) and .entries[0].target_name == "#automod-test"
  and (.entries[0].details | startswith("Blocked word (d*****t)")) and (.entries[0].details | length) < 120
  and any(.actions[]; .key == "automod_block")' >/dev/null || { call GET "/api/guilds/$GUILD/audit-log?action=automod_block" | jq '.entries[0]'; exit 1; }
call GET "/api/guilds/$GUILD/audit-log?action=automod_update" | jq -e '.entries | length >= 3' >/dev/null
# (Alerts are posted in the background: wait until the last one is complete, embed and all.)
for i in $(seq 1 20); do call GET "/api/channels/$AML/messages" | jq -e 'length > 0 and (.[-1].embeds[0].title // "" | test("Automod blocked"))' >/dev/null && break; sleep 0.5; done
call GET "/api/channels/$AML/messages" | jq -e '.[-1].author_bot and .[-1].author_display_name == "Automod"
  and (.[-1].embeds[0].title | test("Automod blocked")) and any(.[-1].embeds[0].fields[]; .name == "Rule")' >/dev/null
# "Hide" lets it through behind a Filtered reveal.
call PUT "/api/channels/$AM/automod" '{"word_filter":true,"words":["dang it"],"word_action":"hide"}' >/dev/null
TOKEN=$TOKEN_D
call POST "/api/channels/$AM/messages" '{"content":"dang it, hidden"}' | jq -e '.filtered == true' >/dev/null
call POST "/api/channels/$AM/messages" '{"content":"not hidden"}' | jq -e '(.filtered // false) == false' >/dev/null
call GET "/api/channels/$AM/messages" | jq -e '[.[] | select(.filtered)] | length == 1' >/dev/null

step "automod: spam limits (rate, repeats, mentions, links, attachments) and auto-timeout"
TOKEN=$TOKEN_A
SP=$(call POST "/api/guilds/$GUILD/channels" '{"name":"spam-test"}' | jq -r .id)
call PUT "/api/channels/$SP/automod" '{"spam_filter":true,"max_messages":3,"per_seconds":10,"block_duplicates":false,"max_mentions":2,"max_links":1,"max_attachments":1}' >/dev/null
TOKEN=$TOKEN_D
blocked '{"content":"<@'"$ALICE"'> <@'"$BOB"'> @here"}' "/api/channels/$SP/messages"
grep -q "Too many mentions" /tmp/body
blocked '{"content":"https://a.example and https://b.example"}' "/api/channels/$SP/messages"
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_D" -F "content=two files" -F "files=@/tmp/pixel.png;type=image/png" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$SP/messages/upload")
[ "$code" = 403 ] && grep -q "Too many attachments" /tmp/body || { echo "attachments -> $code"; cat /tmp/body; exit 1; }
call POST "/api/channels/$SP/messages" '{"content":"one <@'"$ALICE"'>"}' >/dev/null
call POST "/api/channels/$SP/messages" '{"content":"two https://a.example"}' >/dev/null
call POST "/api/channels/$SP/messages" '{"content":"three"}' >/dev/null
blocked '{"content":"four, too fast"}' "/api/channels/$SP/messages"
grep -q "too fast" /tmp/body
TOKEN=$TOKEN_A
DUP=$(call POST "/api/guilds/$GUILD/channels" '{"name":"dup-test"}' | jq -r .id)
call PUT "/api/channels/$DUP/automod" '{"spam_filter":true,"max_messages":0,"block_duplicates":true,"max_mentions":0,"max_links":1,"timeout_minutes":5,"timeout_after":3}' >/dev/null
TOKEN=$TOKEN_D
call POST "/api/channels/$DUP/messages" '{"content":"buy gold"}' >/dev/null
call POST "/api/channels/$DUP/messages" '{"content":"Buy  GOLD"}' >/dev/null
blocked '{"content":"buy gold"}' "/api/channels/$DUP/messages"          # strike 1
grep -q "same message" /tmp/body
blocked '{"content":"https://x.example https://y.example"}' "/api/channels/$DUP/messages"   # strike 2
blocked '{"content":"https://x.example https://z.example"}' "/api/channels/$DUP/messages"   # strike 3: timed out
grep -q "timed out for 5 minutes" /tmp/body
TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/members" | jq -e --arg d "$DORA_ID" '.[] | select(.id == $d) | .timeout_until != null' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=automod_timeout" | jq -e --arg d "$DORA_ID" '.entries[0].target_id == $d and (.entries[0].details | test("5 minutes"))' >/dev/null
TOKEN=$TOKEN_D; [ "$(status POST "/api/channels/$DUP/messages" '{"content":"let me talk"}')" = 403 ]
TOKEN=$TOKEN_A; call POST "/api/guilds/$GUILD/members/$DORA_ID/timeout" '{"minutes":0}' >/dev/null
TOKEN=$TOKEN_D; call POST "/api/channels/$DUP/messages" '{"content":"back again"}' >/dev/null

step "threads: auto-archive by the job, by posting, manual archive and lock"
TOKEN=$TOKEN_A
TC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"archive-test"}' | jq -r .id)
call GET "/api/channels/$TC/thread-defaults" | jq -e '.auto_archive_duration == 4320' >/dev/null
call PUT "/api/channels/$TC/thread-defaults" '{"auto_archive_duration":1440}' | jq -e '.auto_archive_duration == 1440' >/dev/null
fail_call PUT "/api/channels/$TC/thread-defaults" '{"auto_archive_duration":5}'
TOKEN=$TOKEN_B; fail_call PUT "/api/channels/$TC/thread-defaults" '{"auto_archive_duration":60}'; TOKEN=$TOKEN_A
TS=$(call POST "/api/channels/$TC/messages" '{"content":"Loot rules?"}' | jq -r .id)
T1=$(call POST "/api/channels/$TC/threads" '{"name":"Quiet one","message_id":"'"$TS"'"}')
jq -e '.auto_archive_duration == 1440 and (.archived | not) and (.locked | not)' <<<"$T1" >/dev/null; T1=$(jq -r .id <<<"$T1")
T2=$(call POST "/api/channels/$TC/threads" '{"name":"Hourly","auto_archive_duration":60}' | jq -r .id)
fail_call POST "/api/channels/$TC/threads" '{"name":"Odd","auto_archive_duration":30}'
call GET "/api/threads/$T2" | jq -e '.auto_archive_duration == 60' >/dev/null
# Older threads (from before this feature) got the 3-day default.
call GET "/api/channels/$CHAN/threads" | jq -e 'all(.[]; .auto_archive_duration == 4320)' >/dev/null
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-arch.jsonl 60 & ARCH_PROBE=$!
wait_for /tmp/gw-arch.jsonl '.t == "READY"'
psql "$DATABASE_URL" -qc "UPDATE threads SET active_at = now() - interval '2 hours' WHERE id IN ($T1, $T2)"
for i in $(seq 1 40); do call GET "/api/threads/$T2" | jq -e '.archived' >/dev/null && break; sleep 1; done
call GET "/api/threads/$T2" | jq -e '.archived' >/dev/null || { echo "the archiver never archived the thread"; exit 1; }
call GET "/api/threads/$T1" | jq -e '.archived | not' >/dev/null
wait_for /tmp/gw-arch.jsonl '.t == "THREAD_UPDATE" and .d.id == $t and .d.archived' --arg t "$T2"
call GET "/api/channels/$TC/threads" | jq -e --arg t "$T2" '.[-1].id == $t and .[-1].archived' >/dev/null
# Posting in it brings it back (and resets the clock).
TOKEN=$TOKEN_B
call POST "/api/channels/$TC/messages" '{"content":"still here","thread_id":"'"$T2"'"}' >/dev/null
call GET "/api/threads/$T2" | jq -e '(.archived | not) and .message_count == 1' >/dev/null
# Archive by hand: whoever started it or moderators; unarchive: anyone who can post.
fail_call PATCH "/api/threads/$T1" '{"archived":true}'
TOKEN=$TOKEN_A
call PATCH "/api/threads/$T1" '{"archived":true}' | jq -e '.archived' >/dev/null
call GET "/api/channels/$TC/messages" | jq -e --arg m "$TS" '.[] | select(.id == $m) | .thread.archived and .thread.name == "Quiet one"' >/dev/null
TOKEN=$TOKEN_B
call PATCH "/api/threads/$T1" '{"archived":false}' | jq -e '.archived | not' >/dev/null
fail_call PATCH "/api/threads/$T1" '{"locked":true}'
fail_call PATCH "/api/threads/$T1" '{"auto_archive_duration":60}'
# Locked threads stay archived for everyone but moderators.
TOKEN=$TOKEN_A
call PATCH "/api/threads/$T1" '{"archived":true,"locked":true,"auto_archive_duration":10080}' | jq -e '.archived and .locked and .auto_archive_duration == 10080' >/dev/null
TOKEN=$TOKEN_B
[ "$(status POST "/api/channels/$TC/messages" '{"content":"let me in","thread_id":"'"$T1"'"}')" = 400 ] && grep -q "locked" /tmp/body
[ "$(status PATCH "/api/threads/$T1" '{"archived":false}')" = 400 ]
call GET "/api/channels/$TC/messages?thread=$T1" >/dev/null   # still readable
TOKEN=$TOKEN_A
call POST "/api/channels/$TC/messages" '{"content":"mods can","thread_id":"'"$T1"'"}' >/dev/null
call GET "/api/threads/$T1" | jq -e '(.archived | not) and .locked' >/dev/null
call PATCH "/api/threads/$T1" '{"locked":false}' | jq -e '.locked | not' >/dev/null
kill $ARCH_PROBE 2>/dev/null || true
call DELETE "/api/bots/$CB_ID" >/dev/null

step "forwarded spoilers, thread send-later, blocked people in voice, filtered search results"
TOKEN=$TOKEN_A
SPC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"spoiler-talk"}' | jq -r .id)
call PATCH "/api/channels/$SPC" '{"content_mode":"spoiler"}' >/dev/null
SPM=$(call POST "/api/channels/$SPC/messages" '{"content":"the twist: it was the innkeeper"}' | jq -r .id)
LOUT=$(call POST "/api/guilds/$GUILD/channels" '{"name":"lounge"}' | jq -r .id)
SF=$(call POST "/api/channels/$SPC/messages/$SPM/forward" '{"destinations":["'"$LOUT"'"]}')
jq -e '.[0].forward.source_spoiler == true' <<<"$SF" >/dev/null
SF1=$(jq -r '.[0].id' <<<"$SF")
call GET "/api/channels/$LOUT/messages?limit=5" | jq -e --arg f "$SF1" '.[] | select(.id == $f) | .forward.source_spoiler' >/dev/null
# Forwarded on again from an ordinary channel: still a spoiler.
call POST "/api/channels/$LOUT/messages/$SF1/forward" '{"destinations":["'"$CHAN"'"]}' | jq -e '.[0].forward.source_spoiler' >/dev/null
# Ordinary forwards aren't.
NSM=$(call POST "/api/channels/$LOUT/messages" '{"content":"no secrets here"}' | jq -r .id)
call POST "/api/channels/$LOUT/messages/$NSM/forward" '{"destinations":["'"$CHAN"'"]}' | jq -e '.[0].forward.source_spoiler == false' >/dev/null
TOKEN=$OUT; fail_call POST "/api/channels/$SPC/messages/$SPM/forward" '{"destinations":["'"$LOUT"'"]}'; TOKEN=$TOKEN_A
# Thread replies can be scheduled (the thread must be in that channel).
LTH=$(call POST "/api/channels/$LOUT/threads" '{"name":"Plans"}' | jq -r .id)
TLATER=$(date -u -d '+2 hours' +%Y-%m-%dT%H:%M:%SZ)
TS1=$(call POST "/api/channels/$LOUT/scheduled" '{"content":"thread reminder","send_at":"'"$TLATER"'","thread_id":"'"$LTH"'"}')
jq -e --arg t "$LTH" '.thread_id == $t' <<<"$TS1" >/dev/null
call GET /api/users/@me/scheduled | jq -e --arg t "$LTH" 'any(.[]; .thread_id == $t)' >/dev/null
call POST "/api/scheduled/$(jq -r .id <<<"$TS1")/send" | jq -e --arg t "$LTH" '.thread_id == $t and .content == "thread reminder"' >/dev/null
call GET "/api/channels/$LOUT/messages?thread=$LTH" | jq -e 'any(.[]; .content == "thread reminder")' >/dev/null
fail_call POST "/api/channels/$CHAN/scheduled" '{"content":"wrong place","send_at":"'"$TLATER"'","thread_id":"'"$LTH"'"}'
TOKEN=$OUT; fail_call POST "/api/channels/$LOUT/scheduled" '{"content":"not mine","send_at":"'"$TLATER"'","thread_id":"'"$LTH"'"}'; TOKEN=$TOKEN_A
# Search results carry automod's Filtered flag (the app covers them).
call GET "/api/guilds/$GUILD/search?q=hidden" | jq -e 'any(.messages[]; .filtered and (.content | test("dang it")))' >/dev/null

# Blocking in voice: someone who blocked you isn't in your voice lists or events.
node ci/gateway_probe.mjs "$BASE" "$TOKEN_A" /tmp/gw-vb-alice.jsonl 40 &
VB_A=$!
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-vb-bob.jsonl 40 &
VB_B=$!
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-vb-alice.jsonl 2>/dev/null && grep -q '"READY"' /tmp/gw-vb-bob.jsonl 2>/dev/null && break; sleep 0.25; done
TOKEN=$TOKEN_B; call POST "/api/channels/$VOICE/voice/join" >/dev/null
wait_for /tmp/gw-vb-alice.jsonl '.t == "VOICE_STATE_UPDATE" and .d.user_id == $b and .d.channel_id == $c' --arg b "$BOB" --arg c "$VOICE"
TOKEN=$TOKEN_A; call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" 'any(.[]; .user_id == $b)' >/dev/null
# Bob blocks Alice: he drops out of her list at once; Carl still sees him.
TOKEN=$TOKEN_B; call PUT "/api/users/@me/blocks/$ALICE" >/dev/null
wait_for /tmp/gw-vb-alice.jsonl '.t == "VOICE_STATE_UPDATE" and .d.user_id == $b and .d.channel_id == null' --arg b "$BOB"
TOKEN=$TOKEN_A; call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" 'all(.[]; .user_id != $b)' >/dev/null
TOKEN=$TOKEN_C; call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" 'any(.[]; .user_id == $b)' >/dev/null
# His changes don't reach her.
TOKEN=$TOKEN_B; call POST /api/voice/state '{"muted":true,"deafened":false,"streaming":false}' >/dev/null
# Carl joins with an entrance sound after blocking Alice: Bob hears it, Alice doesn't.
TOKEN=$TOKEN_C
call PUT /api/users/@me/entrance-sound '{"builtin":"door"}' >/dev/null
call PUT "/api/users/@me/blocks/$ALICE" >/dev/null
call POST "/api/channels/$VOICE/voice/join" >/dev/null
wait_for /tmp/gw-vb-bob.jsonl '.t == "ENTRANCE_SOUND" and .d.user_id == $c' --arg c "$CARL"
wait_for /tmp/gw-vb-bob.jsonl '.t == "VOICE_STATE_UPDATE" and .d.user_id == $b and .d.muted' --arg b "$BOB"
sleep 1
jq -e -s --arg b "$BOB" --arg c "$CARL" '
  ([.[] | select(.t == "VOICE_STATE_UPDATE" and .d.user_id == $b and .d.muted)] | length == 0)
  and ([.[] | select(.t == "ENTRANCE_SOUND" and .d.user_id == $c)] | length == 0)
  and ([.[] | select(.t == "VOICE_STATE_UPDATE" and .d.user_id == $c and .d.channel_id != null)] | length == 0)' /tmp/gw-vb-alice.jsonl >/dev/null \
  || { echo "Alice was told about people who blocked her:"; cat /tmp/gw-vb-alice.jsonl | tail -5; exit 1; }
# Unblocking: Bob's back in her list straight away (as he is now: muted).
TOKEN=$TOKEN_B; call DELETE "/api/users/@me/blocks/$ALICE" >/dev/null
wait_for /tmp/gw-vb-alice.jsonl '.t == "VOICE_STATE_UPDATE" and .d.user_id == $b and .d.channel_id == $c and .d.muted' --arg b "$BOB" --arg c "$VOICE"
TOKEN=$TOKEN_A; call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" --arg c "$CARL" 'any(.[]; .user_id == $b) and all(.[]; .user_id != $c)' >/dev/null
TOKEN=$TOKEN_C; call DELETE "/api/users/@me/blocks/$ALICE" >/dev/null; call DELETE /api/users/@me/entrance-sound >/dev/null; call POST /api/voice/leave >/dev/null
TOKEN=$TOKEN_B; call POST /api/voice/leave >/dev/null
TOKEN=$TOKEN_A
kill $VB_A $VB_B 2>/dev/null || true

# ---------------------------------------------------------------------------
# Safety: reports, the site owner's console, suspensions, teen protections
# (feat-reports). Alice is the site owner here (the first account).
# ---------------------------------------------------------------------------
UPLOADS=${UPLOAD_DIR:-data/uploads}
xreg() { # xreg N JSON -> HTTP status (body in /tmp/body); each N its own address
  curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.7.$1" -d "$2" "$BASE/api/auth/register"
}
tok() { local t=$1; shift; TOKEN=$t "$@"; } # tok TOKEN call|fail_call|status ...
fupload() { # fupload TOKEN CHANNEL TEXT -> message JSON in /tmp/body
  local c; c=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $1" -F "content=$3" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$2/messages/upload")
  [[ "$c" == 2* ]] || { echo "upload -> $c"; cat /tmp/body; exit 1; }
}
report() { # report TOKEN KIND ID CATEGORY [DETAILS] -> HTTP status (body in /tmp/body)
  tok "$1" status POST /api/reports "$(jq -nc --arg k "$2" --arg i "$3" --arg c "$4" --arg d "${5:-}" '{target_kind:$k,target_id:$i,category:$c,details:$d}')"
}
report_id() { # report_id KIND ID -> the open report about it (as the owner)
  psql "$DATABASE_URL" -tAc "SELECT id FROM reports WHERE target_kind = '$1' AND target_id = $2 AND status <> 'closed'"
}
act() { # act REPORT JSON -> report detail (as the owner)
  tok "$TOKEN_A" call POST "/api/safety/reports/$1/actions" "$2"
}

step "safety: sign-up asks for a date of birth; only the age group is kept"
TEEN_DOB=$(date -u -d '15 years ago' +%F); KID_DOB=$(date -u -d '10 years ago' +%F)
[ "$(xreg 1 '{"username":"nodob","email":"nodob@example.test","password":"password123"}')" = 400 ] && grep -q "date of birth" /tmp/body
[ "$(xreg 2 '{"username":"kiddo","email":"kiddo@example.test","password":"password123","birth_date":"'"$KID_DOB"'"}')" = 403 ] && grep -q "13 or older" /tmp/body
[ "$(xreg 3 '{"username":"oddone","email":"oddone@example.test","password":"password123","birth_date":"31/12/1990"}')" = 400 ]
[ "$(xreg 4 '{"username":"futurekid","email":"futurekid@example.test","password":"password123","birth_date":"2999-01-01"}')" = 400 ]
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM users WHERE username IN ('nodob','kiddo','oddone','futurekid')" | grep -qx 0
[ "$(xreg 5 '{"username":"tia","email":"tia@example.test","password":"password123","display_name":"Tia","birth_date":"'"$TEEN_DOB"'"}')" = 200 ]
TOKEN_T=$(jq -r .token /tmp/body); TIA=$(jq -r .user.id /tmp/body)
[ "$(xreg 6 '{"username":"adam","email":"adam@example.test","password":"password123","display_name":"Adam","birth_date":"1985-06-01"}')" = 200 ]
TOKEN_AD=$(jq -r .token /tmp/body); ADAM=$(jq -r .user.id /tmp/body)
[ "$(xreg 7 '{"username":"fran","email":"fran@example.test","password":"password123","display_name":"Fran","birth_date":"1979-02-02"}')" = 200 ]
TOKEN_F=$(jq -r .token /tmp/body); FRAN=$(jq -r .user.id /tmp/body)
psql "$DATABASE_URL" -tAc "SELECT age_bracket || ',' || dm_privacy FROM users WHERE id = $TIA" | grep -qx "teen,friends"
psql "$DATABASE_URL" -tAc "SELECT age_bracket FROM users WHERE id = $ADAM" | grep -qx adult
psql "$DATABASE_URL" -tAc "SELECT age_bracket FROM users WHERE id = $ALICE" | grep -qx adult   # older accounts: adults
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM information_schema.columns WHERE table_name = 'users' AND column_name ILIKE '%birth%'" | grep -qx 0

step "safety: teens only hear from people they share an outhouse with (friends of friends or family)"
SG=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Safety Test"}' | jq -r .id)
SCH=$(tok "$TOKEN_A" call GET "/api/guilds/$SG/channels" | jq -r '[.[] | select(.kind == 0)][0].id')
SINV=$(tok "$TOKEN_A" call POST "/api/guilds/$SG/invites" | jq -r .code)
for t in "$TOKEN_T" "$TOKEN_AD" "$TOKEN_F"; do tok "$t" call POST "/api/invites/$SINV" >/dev/null; done
# Adults who aren't their friend can't DM a teen (nor a teen them)...
[ "$(tok "$TOKEN_AD" status POST /api/dms '{"user_ids":["'"$TIA"'"]}')" = 403 ] && grep -q "can't receive messages from you" /tmp/body
tok "$TOKEN_AD" call GET "/api/users/$TIA/contact" | jq -e '(.can_message | not) and .reason == "This person can'"'"'t receive messages from you."' >/dev/null
[ "$(tok "$TOKEN_T" status POST /api/dms '{"user_ids":["'"$ADAM"'"]}')" = 403 ]
tok "$TOKEN_AD" call GET "/api/users/$FRAN/contact" | jq -e '.can_message' >/dev/null
# ...or send them a friend request (no friend in common, no family outhouse),
[ "$(tok "$TOKEN_AD" status PUT "/api/users/@me/relationships/$TIA")" = 403 ]
# and nobody they don't share an outhouse with can.
[ "$(tok "$TOKEN_B" status PUT "/api/users/@me/relationships/$TIA")" = 403 ]
# A family outhouse lets them ask; once friends they can DM.
tok "$TOKEN_B" fail_call PUT "/api/safety/guilds/$SG/family" '{"family":true}'
tok "$TOKEN_A" call PUT "/api/safety/guilds/$SG/family" '{"family":true}' >/dev/null
tok "$TOKEN_AD" call PUT "/api/users/@me/relationships/$TIA" | jq -e '.kind == "outgoing"' >/dev/null
tok "$TOKEN_T" call PUT "/api/users/@me/relationships/$ADAM" | jq -e '.kind == "friend"' >/dev/null
TDM=$(tok "$TOKEN_AD" call POST /api/dms '{"user_ids":["'"$TIA"'"]}' | jq -r .id)
tok "$TOKEN_AD" call POST "/api/channels/$TDM/messages" '{"content":"hi Tia, dinner at 6"}' >/dev/null
tok "$TOKEN_A" call PUT "/api/safety/guilds/$SG/family" '{"family":false}' >/dev/null
# Friends of friends (and sharing an outhouse) may ask too: Fran is Adam's friend.
tok "$TOKEN_F" call PUT "/api/users/@me/relationships/$ADAM" >/dev/null; tok "$TOKEN_AD" call PUT "/api/users/@me/relationships/$FRAN" >/dev/null
tok "$TOKEN_F" call PUT "/api/users/@me/relationships/$TIA" | jq -e '.kind == "outgoing"' >/dev/null
tok "$TOKEN_T" call DELETE "/api/users/@me/relationships/$FRAN" >/dev/null
# Group DMs: every adult in it must be the teen's friend.
[ "$(tok "$TOKEN_AD" status POST /api/dms '{"user_ids":["'"$TIA"'","'"$FRAN"'"]}')" = 403 ]
GDM=$(tok "$TOKEN_AD" call POST /api/dms '{"user_ids":["'"$FRAN"'","'"$ALICE"'"]}' | jq -r .id)
[ "$(tok "$TOKEN_AD" status POST "/api/dms/$GDM/recipients" '{"user_id":"'"$TIA"'"}')" = 403 ]
# An adult the owner later marks as a teen's... existing DM: sending is checked too.
tok "$TOKEN_A" call POST "/api/safety/users/$FRAN/actions" '{"action":"set_age","bracket":"teen"}' | jq -e '.age_bracket == "teen"' >/dev/null
FDM=$(tok "$TOKEN_AD" call POST /api/dms '{"user_ids":["'"$FRAN"'"]}' | jq -r .id)   # friends: fine
tok "$TOKEN_AD" call DELETE "/api/users/@me/relationships/$FRAN" >/dev/null
[ "$(tok "$TOKEN_AD" status POST "/api/channels/$FDM/messages" '{"content":"still there?"}')" = 403 ]
tok "$TOKEN_A" call POST "/api/safety/users/$FRAN/actions" '{"action":"set_age","bracket":"adult"}' >/dev/null
# The owner can switch the limits off (and back on).
tok "$TOKEN_A" call PUT /api/safety/settings '{"teen_contact_limits":false}' | jq -e '(.teen_contact_limits | not) and .teen_block_adult_channels' >/dev/null
tok "$TOKEN_B" call GET "/api/users/$TIA/contact" | jq -e '(.reason // "") | contains("can'"'"'t receive messages from you") | not' >/dev/null
tok "$TOKEN_A" call PUT /api/safety/settings '{"teen_contact_limits":true}' >/dev/null
tok "$TOKEN_B" fail_call GET /api/safety/settings
tok "$TOKEN_B" fail_call PUT /api/safety/settings '{"teen_contact_limits":false}'

step "safety: teens can't open 18+ channels, whatever they click"
NS=$(tok "$TOKEN_A" call POST "/api/guilds/$SG/channels" '{"name":"after-dark"}' | jq -r .id)
tok "$TOKEN_A" call PATCH "/api/channels/$NS" '{"content_mode":"nsfw"}' >/dev/null
tok "$TOKEN_AD" call PUT "/api/channels/$NS/age-gate" >/dev/null
tok "$TOKEN_AD" call GET "/api/channels/$NS/messages" >/dev/null
[ "$(tok "$TOKEN_T" status PUT "/api/channels/$NS/age-gate")" = 403 ] && grep -q "adults (18+) only" /tmp/body
[ "$(tok "$TOKEN_T" status GET "/api/channels/$NS/messages")" = 403 ]
# Even an old "I'm 18+" from before doesn't count while the protection is on.
tok "$TOKEN_A" call PUT /api/safety/settings '{"teen_block_adult_channels":false}' >/dev/null
tok "$TOKEN_T" call PUT "/api/channels/$NS/age-gate" >/dev/null
tok "$TOKEN_T" call GET "/api/channels/$NS/messages" >/dev/null
tok "$TOKEN_A" call PUT /api/safety/settings '{"teen_block_adult_channels":true}' >/dev/null
[ "$(tok "$TOKEN_T" status GET "/api/channels/$NS/messages")" = 403 ]

step "safety: report a message, dedupe, anonymity, owner-only console"
SINV2=$(tok "$TOKEN_A" call POST "/api/guilds/$GUILD/invites" | jq -r .code)
tok "$TOKEN_T" call POST "/api/invites/$SINV2" >/dev/null
M1=$(tok "$TOKEN_B" call POST "/api/channels/$CHAN/messages" '{"content":"cheap gold at scam.example, DM me"}' | jq -r .id)
[ "$(report "$TOKEN_D" message "$M1" spam "selling gold again")" = 201 ] && jq -e '.ok and (.duplicate | not)' /tmp/body >/dev/null
[ "$(report "$TOKEN_D" message "$M1" spam)" = 200 ] && jq -e '.duplicate' /tmp/body >/dev/null   # once per person
[ "$(report "$TOKEN_B" message "$M1" spam)" = 400 ]                                                # not your own
[ "$(report "$OUT" message "$M1" spam)" = 404 ]                                                    # can't see it
[ "$(report "$TOKEN_D" message "$M1" nonsense)" = 400 ]
[ "$(report "$TOKEN_D" message "$M1" spam "$(head -c 1001 /dev/zero | tr '\0' x)")" = 400 ]
[ "$(report "$TOKEN_T" message "$M1" harassment)" = 200 ]                                          # teens can report too
R1=$(report_id message "$M1")
for path in /api/safety/summary "/api/safety/reports" "/api/safety/reports/$R1" "/api/safety/reports/$R1/export" \
            /api/safety/users "/api/safety/users/$BOB" /api/safety/log /api/safety/digest; do
  tok "$TOKEN_B" fail_call GET "$path"; tok "$TOKEN_D" fail_call GET "$path"
done
tok "$TOKEN_B" fail_call POST "/api/safety/reports/$R1/actions" '{"action":"dismiss"}'
tok "$TOKEN_B" fail_call POST "/api/safety/users/$DORA_ID/actions" '{"action":"suspend"}'
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg r "$R1" --arg b "$BOB" '
  .reports[] | select(.id == $r) | .report_count == 2 and .category == "spam" and .reported_user.id == $b
  and (.place | test("#general")) and (.preview | test("cheap gold"))' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$R1" | jq -e --arg d "$DORA_ID" '
  (.reporters | length) == 2 and .reporters[0].id == $d and .reporters[0].details == "selling gold again"
  and .snapshot.message.content == "cheap gold at scam.example, DM me" and .subject.username == "bob"
  and (.snapshot.context | length) >= 1 and (.snapshot.context | length) <= 20' >/dev/null

step "safety: the snapshot survives edits and deletion; files are kept until the report closes"
tok "$TOKEN_B" call PATCH "/api/channels/$CHAN/messages/$M1" '{"content":"nothing to see here"}' >/dev/null
tok "$TOKEN_B" call DELETE "/api/channels/$CHAN/messages/$M1" >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$R1" | jq -e --arg b "$BOB" '
  .snapshot.message.content == "cheap gold at scam.example, DM me" and .snapshot.edits[0].before == "cheap gold at scam.example, DM me"
  and .snapshot.edits[0].after == "nothing to see here" and .snapshot.deleted.by == $b and (.message_live | not)' >/dev/null
fupload "$TOKEN_B" "$CHAN" "rude picture"; M2=$(jq -r .id /tmp/body); K2=$(jq -r '.attachments[0].url | split("/")[2]' /tmp/body)
[ "$(report "$TOKEN_D" message "$M2" harassment)" = 201 ]; R2=$(report_id message "$M2")
tok "$TOKEN_B" call DELETE "/api/channels/$CHAN/messages/$M2" >/dev/null
[ -f "$UPLOADS/$K2" ] || { echo "a reported file was deleted with its message"; exit 1; }
tok "$TOKEN_A" call GET "/api/safety/reports/$R2" | jq -e --arg k "$K2" '.files[0].key == $k and .files[0].viewable and .files[0].on_disk and (.files[0].sha256 | length) == 64' >/dev/null
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$R2/files/$K2")" = 200 ]
[ "$(tok "$TOKEN_B" status GET "/api/safety/reports/$R2/files/$K2")" = 403 ]
act "$R2" '{"action":"close","text":"Warned them"}' | jq -e '.report.status == "closed" and .report.resolution == "Warned them"' >/dev/null
[ ! -f "$UPLOADS/$K2" ] || { echo "the file outlived its closed report"; exit 1; }
act "$R2" '{"action":"reopen"}' | jq -e '.report.status == "review"' >/dev/null
act "$R2" '{"action":"close"}' >/dev/null

step "safety: every kind of thing can be reported (files, DMs, threads, people, stickers, emoji, outhouses)"
fupload "$TOKEN_B" "$CHAN" "a file"; A3=$(jq -r '.attachments[0].id' /tmp/body)
[ "$(report "$TOKEN_D" attachment "$A3" sexual)" = 201 ]
tok "$TOKEN_A" call GET "/api/safety/reports/$(report_id attachment "$A3")" | jq -e --arg a "$A3" '.snapshot.focus_attachment_id == $a and (.files | length) == 1 and .report.priority == 1' >/dev/null
DDM=$(tok "$TOKEN_D" call POST /api/dms '{"user_ids":["'"$BOB"'"]}' | jq -r .id)
DM1=$(tok "$TOKEN_B" call POST "/api/channels/$DDM/messages" '{"content":"send me pics"}' | jq -r .id)
[ "$(report "$TOKEN_D" message "$DM1" harassment)" = 201 ]
[ "$(report "$TOKEN_A" message "$DM1" harassment)" = 404 ]   # not in that DM
tok "$TOKEN_A" call GET "/api/safety/reports/$(report_id message "$DM1")" | jq -e '.snapshot.place.dm and (.snapshot.place.participants | length) == 2 and .report.place == "Direct message"' >/dev/null
RT=$(tok "$TOKEN_A" call POST "/api/channels/$CHAN/threads" '{"name":"Report thread"}' | jq -r .id)
TM=$(tok "$TOKEN_B" call POST "/api/channels/$CHAN/messages" '{"content":"thread nastiness","thread_id":"'"$RT"'"}' | jq -r .id)
[ "$(report "$TOKEN_D" message "$TM" hate)" = 201 ]
tok "$TOKEN_A" call GET "/api/safety/reports/$(report_id message "$TM")" | jq -e '.snapshot.place.thread_name == "Report thread" and .report.priority == 1' >/dev/null
[ "$(report "$TOKEN_D" user "$BOB" impersonation "pretending to be the GM")" = 201 ]
[ "$(report "$TOKEN_D" user "$DORA_ID" spam)" = 400 ]       # not yourself
supload "$TOKEN_A" /tmp/pixel.png "Report me"; RSTK=$(jq -r .id /tmp/body)
[ "$(report "$TOKEN_D" sticker "$RSTK" other)" = 201 ]
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "name=reportme" -F "file=@/tmp/pixel.png;type=image/png" "$BASE/api/guilds/$GUILD/emojis")
[[ "$code" == 2* ]] || { echo "emoji upload -> $code"; cat /tmp/body; exit 1; }; REMO=$(jq -r .id /tmp/body)
[ "$(report "$TOKEN_D" emoji "$REMO" hate)" = 201 ]
[ "$(report "$TOKEN_D" guild "$GUILD" other "whole place is a mess")" = 201 ]
[ "$(report "$TOKEN_T" guild "$IGUILD" other)" = 404 ]      # not a member
[ "$(report "$TOKEN_D" message 12345 spam)" = 404 ]
[ "$(report "$TOKEN_D" banana "$BOB" spam)" = 400 ]
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e '[.reports[].target_kind] | (index("user") and index("sticker") and index("emoji") and index("guild") and index("attachment"))' >/dev/null
# The sticker's file is kept while the report is open, even if the sticker goes.
SKEY=$(psql "$DATABASE_URL" -tAc "SELECT storage_key FROM stickers WHERE id = $RSTK")
tok "$TOKEN_A" call DELETE "/api/stickers/$RSTK" >/dev/null
[ -f "$UPLOADS/$SKEY" ] || { echo "reported sticker's file was deleted"; exit 1; }

step "safety: child-safety reports quarantine files (451 for everyone), hide the message and alert the owner"
node ci/gateway_probe.mjs "$BASE" "$TOKEN_A" /tmp/gw-safety-owner.jsonl 60 & SP1=$!
node ci/gateway_probe.mjs "$BASE" "$TOKEN_D" /tmp/gw-safety-dora.jsonl 60 & SP2=$!
wait_for /tmp/gw-safety-owner.jsonl '.t == "READY"'; wait_for /tmp/gw-safety-dora.jsonl '.t == "READY"'
fupload "$TOKEN_B" "$CHAN" "look at this"; M3=$(jq -r .id /tmp/body); K3=$(jq -r '.attachments[0].url | split("/")[2]' /tmp/body); U3=$(jq -r '.attachments[0].url' /tmp/body)
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U3")" = 200 ]
[ "$(report "$TOKEN_D" message "$M3" child_safety "an image of a child")" = 201 ]
for who in "" "$TOKEN_A" "$TOKEN_B" "$TOKEN_D"; do
  hdr=(); [ -n "$who" ] && hdr=(-H "Authorization: Bearer $who")
  code=$(curl -sS -o /tmp/body -w '%{http_code}' ${hdr[@]+"${hdr[@]}"} "$BASE$U3")
  [ "$code" = 451 ] && grep -q "removed pending review" /tmp/body || { echo "quarantined file -> $code"; exit 1; }
done
[ -f "$UPLOADS/$K3" ] || { echo "quarantined file isn't kept on disk"; exit 1; }
for t in "$TOKEN_A" "$TOKEN_B" "$TOKEN_D"; do
  tok "$t" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$M3" 'all(.[]; .id != $m)' >/dev/null
done
[ "$(tok "$TOKEN_B" status PATCH "/api/channels/$CHAN/messages/$M3" '{"content":"edit it away"}')" = 404 ]
wait_for /tmp/gw-safety-dora.jsonl '.t == "MESSAGE_DELETE" and .d.id == $m' --arg m "$M3"
wait_for /tmp/gw-safety-owner.jsonl '.t == "REPORTS_CHANGED" and .d.alert and .d.urgent >= 1'
for i in $(seq 1 20); do [ "$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)" -ge 1 ] && break; sleep 0.5; done
curl -fsS "$PUSH/alerts/safety-ntfy" | jq -e '.[0].title == "Outhouse: urgent safety report" and .[0].priority == "high" and (.[0].body | test("child-safety")) and (.[0].body | test("look at this") | not)' >/dev/null
curl -fsS "$PUSH/alerts/safety-hook" | jq -e '.[0].body | fromjson | .content | test("urgent safety report")' >/dev/null
R3=$(report_id message "$M3")
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg r "$R3" '.reports[0].id == $r and .reports[0].child_safety and .counts.urgent >= 1' >/dev/null
SHA3=$(sha256sum "$UPLOADS/$K3" | cut -d' ' -f1)
tok "$TOKEN_A" call GET "/api/safety/reports/$R3" | jq -e --arg h "$SHA3" '.report.child_safety and .message_hidden
  and .files[0].quarantined and (.files[0].viewable | not) and .files[0].sha256 == $h and .snapshot.message.content == "look at this"' >/dev/null
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$R3/files/$K3")" = 451 ]
code=$(curl -sS -D /tmp/headers -o /tmp/export.json -w '%{http_code}' -H "Authorization: Bearer $TOKEN_A" "$BASE/api/safety/reports/$R3/export")
[ "$code" = 200 ] && grep -qi 'content-disposition: attachment; filename="outhouse-report-' /tmp/headers
jq -e --arg h "$SHA3" --arg k "$K3" '.format == "outhouse-evidence-package/1" and .files[0].sha256 == $h and .files[0].storage_key == null
  and .files[0].key == $k and (.files_note | test("not in this package")) and .reported_account.username == "bob" and (.reporters | length) == 1' /tmp/export.json >/dev/null
# Escalation: a spam report becomes child safety when someone says so (and gets quarantined).
fupload "$TOKEN_B" "$CHAN" "another one"; M4=$(jq -r .id /tmp/body); U4=$(jq -r '.attachments[0].url' /tmp/body)
[ "$(report "$TOKEN_D" message "$M4" spam)" = 201 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U4")" = 200 ]
[ "$(report "$TOKEN_T" message "$M4" child_safety)" = 200 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U4")" = 451 ]
R4=$(report_id message "$M4")
tok "$TOKEN_A" call GET "/api/safety/reports/$R4" | jq -e '.report.category == "child_safety" and .report.priority == 0 and .report.report_count == 2' >/dev/null
# Dismissing (a mistake) brings the file and message back.
act "$R4" '{"action":"dismiss","text":"A cat picture"}' | jq -e '.report.status == "closed" and (.files[0].quarantined | not)' >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U4")" = 200 ]
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$M4" 'any(.[]; .id == $m)' >/dev/null
# Closing a real one keeps the evidence (still quarantined, still on disk) until it's deleted on purpose.
tok "$TOKEN_A" fail_call POST "/api/safety/reports/$R3/actions" '{"action":"delete_evidence"}'   # close it first
act "$R3" '{"action":"close","text":"Reported to the ACCCE"}' >/dev/null
[ -f "$UPLOADS/$K3" ] && [ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U3")" = 451 ]
act "$R3" '{"action":"delete_evidence"}' | jq -e '.report.evidence_released and (.files[0].on_disk | not) and (.files[0].sha256 | length) == 64' >/dev/null
[ ! -f "$UPLOADS/$K3" ] && [ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$U3")" = 451 ]
kill $SP1 $SP2 2>/dev/null || true

step "safety: rate limit per reporter"
[ "$(xreg 8 '{"username":"rachel","email":"rachel@example.test","password":"password123","birth_date":"1990-03-03"}')" = 200 ]
TOKEN_R=$(jq -r .token /tmp/body)
tok "$TOKEN_R" call POST "/api/invites/$SINV2" >/dev/null
RL=()
for i in $(seq 1 21); do RL+=("$(tok "$TOKEN_B" call POST "/api/channels/$CHAN/messages" '{"content":"spam '"$i"'"}' | jq -r .id)"); done
for i in $(seq 0 19); do [ "$(report "$TOKEN_R" message "${RL[$i]}" spam)" = 201 ]; done
[ "$(report "$TOKEN_R" message "${RL[20]}" spam)" = 429 ] && grep -q "000" /tmp/body
psql "$DATABASE_URL" -qc "UPDATE reports SET status = 'closed', closed_at = now(), resolution = 'smoke test' WHERE id IN (SELECT report_id FROM report_reporters WHERE reporter_id = (SELECT id FROM users WHERE username = 'rachel'))"

step "safety: owner actions (delete, remove files, warn, timeout, kick, ban, notes)"
M5=$(tok "$TOKEN_B" call POST "/api/channels/$CHAN/messages" '{"content":"delete me please"}' | jq -r .id)
[ "$(report "$TOKEN_D" message "$M5" harassment)" = 201 ]; R5=$(report_id message "$M5")
act "$R5" '{"action":"delete_message"}' | jq -e '(.message_live | not) and .report.status == "review" and .snapshot.message.content == "delete me please"' >/dev/null
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$M5" 'all(.[]; .id != $m)' >/dev/null
fupload "$TOKEN_B" "$CHAN" "keep the words"; M6=$(jq -r .id /tmp/body)
[ "$(report "$TOKEN_D" message "$M6" sexual)" = 201 ]; R6=$(report_id message "$M6")
act "$R6" '{"action":"remove_attachment"}' >/dev/null
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$M6" '.[] | select(.id == $m) | (.attachments | length) == 0 and .content == "keep the words"' >/dev/null
act "$R6" '{"action":"warn","text":"Please keep pictures family-friendly."}' >/dev/null
SBOT=$(psql "$DATABASE_URL" -tAc "SELECT safety_bot_id FROM site_settings")
WDM=$(tok "$TOKEN_B" call GET /api/dms | jq -r --arg s "$SBOT" '.[] | select(any(.recipients[]; .id == $s)) | .id')
tok "$TOKEN_B" call GET "/api/channels/$WDM/messages" | jq -e '.[-1].author_display_name == "Outhouse Safety" and (.[-1].content | test("family-friendly"))
  and (.[-1].content | test("Dora|dora") | not)' >/dev/null
act "$R6" '{"action":"timeout","guild_id":"'"$GUILD"'","minutes":10}' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$GUILD/members" | jq -e --arg b "$BOB" '.[] | select(.id == $b) | .timeout_until != null' >/dev/null
tok "$TOKEN_A" call POST "/api/guilds/$GUILD/members/$BOB/timeout" '{"minutes":0}' >/dev/null
act "$R6" '{"action":"note","text":"Second time this month."}' | jq -e 'any(.log[]; .action == "note" and .details == "Second time this month.") and any(.log[]; .action == "warn")' >/dev/null
[ "$(xreg 9 '{"username":"kev","email":"kev@example.test","password":"password123","birth_date":"1990-04-04"}')" = 200 ]
TOKEN_K=$(jq -r .token /tmp/body); KEV=$(jq -r .user.id /tmp/body)
tok "$TOKEN_K" call POST "/api/invites/$SINV2" >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$KEV/actions" '{"action":"kick","guild_id":"'"$GUILD"'"}' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$GUILD/members" | jq -e --arg k "$KEV" 'all(.[]; .id != $k)' >/dev/null
tok "$TOKEN_K" call POST "/api/invites/$SINV2" >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$KEV/actions" '{"action":"ban","guild_id":"'"$GUILD"'","text":"spam bot"}' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$GUILD/bans" | jq -e --arg k "$KEV" 'any(.[]; .id == $k and (.reason | test("spam bot")))' >/dev/null
tok "$TOKEN_K" fail_call POST "/api/invites/$SINV2"
tok "$TOKEN_A" call DELETE "/api/guilds/$GUILD/bans/$KEV" >/dev/null
tok "$TOKEN_A" fail_call POST "/api/safety/users/$ALICE/actions" '{"action":"suspend"}'           # not yourself
tok "$TOKEN_A" call GET "/api/guilds/$GUILD/audit-log?action=member_ban" | jq -e --arg k "$KEV" '.entries[0].target_id == $k and (.entries[0].details | test("Outhouse Safety"))' >/dev/null

step "safety: suspending an account signs it out everywhere, stops its bots and hides its profile"
[ "$(xreg 10 '{"username":"sam","email":"sam@example.test","password":"password123","display_name":"Sam","birth_date":"1990-05-05"}')" = 200 ]
TOKEN_S=$(jq -r .token /tmp/body); SAM=$(jq -r .user.id /tmp/body)
tok "$TOKEN_S" call POST "/api/invites/$SINV2" >/dev/null
SAMG=$(tok "$TOKEN_S" call POST /api/guilds '{"name":"Sam Place"}' | jq -r .id)
SAMBOT=$(tok "$TOKEN_S" call POST "/api/guilds/$SAMG/bots" '{"name":"Sambot"}' | jq -r .token)
curl -fsS -H "Authorization: Bot $SAMBOT" "$BASE/api/users/@me" >/dev/null
SM1=$(tok "$TOKEN_S" call POST "/api/channels/$CHAN/messages" '{"content":"hello from sam"}' | jq -r .id)
[ "$(report "$TOKEN_D" user "$SAM" harassment)" = 201 ]; RS=$(report_id user "$SAM")
node ci/gateway_probe.mjs "$BASE" "$TOKEN_S" /tmp/gw-sam.jsonl 60 & SP3=$!
wait_for /tmp/gw-sam.jsonl '.t == "READY"'
act "$RS" '{"action":"suspend","days":7,"text":"Harassing people","hide_messages":true}' | jq -e '.subject.suspended and (.subject.suspended_label | startswith("until"))' >/dev/null
wait_for /tmp/gw-sam.jsonl '.t == "INVALID_SESSION"'
for i in $(seq 1 20); do kill -0 $SP3 2>/dev/null || break; sleep 0.25; done
kill -0 $SP3 2>/dev/null && { echo "the suspended account's gateway stayed open"; exit 1; }
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN_S" "$BASE/api/users/@me")" = 401 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bot $SAMBOT" "$BASE/api/users/@me")" = 401 ]
code=$(curl -sS -o /tmp/body -w '%{http_code}' -H "Content-Type: application/json" -d '{"username":"sam","password":"password123"}' "$BASE/api/auth/login")
[ "$code" = 403 ] && grep -q "suspended until" /tmp/body && grep -q "Harassing people" /tmp/body
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$SM1" 'all(.[]; .id != $m)' >/dev/null
[ "$(tok "$TOKEN_D" status GET "/api/profiles/$SAM")" = 404 ]
tok "$TOKEN_A" call GET "/api/profiles/$SAM" >/dev/null
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM sessions WHERE user_id = $SAM" | grep -qx 0
# Permanently, then lifted: they sign in again; messages come back when shown again.
act "$RS" '{"action":"suspend","text":"for good"}' | jq -e '.subject.suspended_label == "permanently"' >/dev/null
act "$RS" '{"action":"unsuspend"}' | jq -e '.subject.suspended | not' >/dev/null
TOKEN_S=$(TOKEN="" call POST /api/auth/login '{"username":"sam","password":"password123"}' | jq -r .token)
tok "$TOKEN_S" call GET /api/users/@me >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bot $SAMBOT" "$BASE/api/users/@me")" = 401 ]   # reset after a suspension
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$SM1" 'all(.[]; .id != $m)' >/dev/null
act "$RS" '{"action":"unhide_messages"}' >/dev/null
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg m "$SM1" 'any(.[]; .id == $m)' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/log" | jq -e '[.entries[].action] | (index("suspend") and index("unsuspend") and index("quarantine") and index("delete_evidence") and index("export"))' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users?q=sam" | jq -e --arg s "$SAM" 'any(.[]; .id == $s)' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$SAM" | jq -e '(.reports | length) == 1 and .bots == 1 and (.ip_note | test("kept 90 days"))' >/dev/null
tok "$TOKEN_A" call GET /api/safety/digest | jq -e 'any(.new_accounts[]; .username == "tia" and .age_bracket == "teen") and .reports_total >= 10' >/dev/null
tok "$TOKEN_A" call GET /api/safety/summary | jq -e '.open >= 1' >/dev/null
# A teen can report and block anyone.
[ "$(report "$TOKEN_T" user "$ADAM" harassment)" = 201 ]
tok "$TOKEN_T" call PUT "/api/users/@me/blocks/$ADAM" >/dev/null

step "safety: child-safety alerts nudge to suspend; Suspend now (until reviewed, neutral reason only), then permanent or lifted"
[ "$(xreg 11 '{"username":"nate","email":"nate@example.test","password":"password123","display_name":"Nate","birth_date":"1990-07-07"}')" = 200 ]
TOKEN_N=$(jq -r .token /tmp/body); NATE=$(jq -r .user.id /tmp/body)
tok "$TOKEN_N" call POST "/api/invites/$SINV2" >/dev/null
NM1=$(tok "$TOKEN_N" call POST "/api/channels/$CHAN/messages" '{"content":"an ordinary message from nate"}' | jq -r .id)
fupload "$TOKEN_N" "$CHAN" "evidence pic"; NM2=$(jq -r .id /tmp/body); KN=$(jq -r '.attachments[0].url | split("/")[2]' /tmp/body); UN=$(jq -r '.attachments[0].url' /tmp/body)
NTFY0=$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)
[ "$(report "$TOKEN_D" message "$NM2" child_safety "he's grooming my cousin")" = 201 ]; RN=$(report_id message "$NM2")
for _ in $(seq 1 20); do [ "$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)" -gt "$NTFY0" ] && break; sleep 0.5; done
curl -fsS "$PUSH/alerts/safety-ntfy" | jq -e '.[-1].body | test("Review and suspend the account now if in doubt") and (test("evidence pic|grooming") | not)' >/dev/null
# The owner's push opens that report and says to suspend if in doubt (no content).
for _ in $(seq 1 20); do curl -fsS "$PUSH/received/alice" | jq -e --arg r "$RN" 'any(.[]; .payload.report_id == $r)' >/dev/null && break; sleep 0.5; done
curl -fsS "$PUSH/received/alice" | jq -e --arg r "$RN" 'any(.[]; .payload.kind == "safety" and .payload.report_id == $r
  and .payload.url == "/app?reports=1&report=\($r)" and (.payload.body | test("suspend the account now if in doubt"))
  and (.payload.body | test("evidence pic|grooming") | not))' >/dev/null
# Nothing happens to the account by itself.
tok "$TOKEN_N" call GET /api/users/@me >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e '(.subject.suspended | not) and .report.child_safety' >/dev/null
node ci/gateway_probe.mjs "$BASE" "$TOKEN_N" /tmp/gw-nate.jsonl 60 & SPN=$!
wait_for /tmp/gw-nate.jsonl '.t == "READY"'
tok "$TOKEN_B" fail_call POST "/api/safety/reports/$RN/actions" '{"action":"suspend_pending"}'
act "$RN" '{"action":"suspend_pending"}' | jq -e --arg a "$ALICE" '.subject.suspended and .subject.suspension_pending
  and .subject.suspended_label == "until reviewed" and .subject.suspended_by == $a and .subject.suspended_at != null
  and (.subject.suspended_permanently | not) and .report.status == "review"' >/dev/null
wait_for /tmp/gw-nate.jsonl '.t == "INVALID_SESSION"'
kill $SPN 2>/dev/null || true
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN_N" "$BASE/api/users/@me")" = 401 ]
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM sessions WHERE user_id = $NATE" | grep -qx 0
# They see only the neutral reason: never the category, the details or who reported them.
code=$(curl -sS -o /tmp/body -w '%{http_code}' -H "Content-Type: application/json" -d '{"username":"nate","password":"password123"}' "$BASE/api/auth/login")
[ "$code" = 403 ] && jq -e '.error == "This account has been suspended pending review of a report."' /tmp/body >/dev/null
grep -qiE "child|safety|groom|dora|permanent" /tmp/body && { echo "the suspension message says too much"; exit 1; }
# Nothing else of theirs is hidden (only the reported message is, as before).
tok "$TOKEN_D" call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg a "$NM1" --arg b "$NM2" 'any(.[]; .id == $a) and all(.[]; .id != $b)' >/dev/null
tok "$TOKEN_A" fail_call POST "/api/safety/reports/$RN/actions" '{"action":"suspend_pending"}'   # already suspended
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e 'any(.log[]; .action == "suspend" and (.details | test("until reviewed")))' >/dev/null
# Make permanent: no longer "pending review".
act "$RN" '{"action":"make_permanent"}' | jq -e '.subject.suspended_label == "permanently" and (.subject.suspension_pending | not) and .subject.suspended_permanently' >/dev/null
code=$(curl -sS -o /tmp/body -w '%{http_code}' -H "Content-Type: application/json" -d '{"username":"nate","password":"password123"}' "$BASE/api/auth/login")
[ "$code" = 403 ] && jq -e '.error == "This account has been suspended permanently."' /tmp/body >/dev/null
# Lifted: they can sign in again.
act "$RN" '{"action":"unsuspend"}' | jq -e '(.subject.suspended | not) and .subject.suspended_at == null' >/dev/null
TOKEN_N=$(TOKEN="" call POST /api/auth/login '{"username":"nate","password":"password123"}' | jq -r .token)
tok "$TOKEN_N" call GET /api/users/@me >/dev/null

step "evidence hand-over: deploy/evidence-handover.sh packs the original files byte for byte, the package and hashes, and logs it"
HO=$(mktemp -d)
OUTHOUSE_CMD=./target/debug/outhouse-server bash deploy/evidence-handover.sh "$RN" "$HO" > /tmp/handover.out 2>&1 || { cat /tmp/handover.out; exit 1; }
PKG=$(ls "$HO"/evidence-"$RN"-*.tar.gz)
[ "$(stat -c %a "$PKG")" = 600 ]
PKG_SHA=$(sha256sum "$PKG" | cut -d' ' -f1)
grep -q "$PKG_SHA" /tmp/handover.out
grep -q "evidence pic\|PNG" /tmp/handover.out && { echo "the hand-over printed file contents"; exit 1; }
HX=$(mktemp -d); tar -C "$HX" -xzf "$PKG"; HD=$(ls -d "$HX"/evidence-"$RN"-*)
( cd "$HD" && sha256sum -c --quiet MANIFEST.sha256 )
cmp "$HD/files/$KN" "$UPLOADS/$KN"
grep -q "files/$KN" "$HD/MANIFEST.sha256" && grep -q "report-$RN.json" "$HD/MANIFEST.sha256"
jq -e --arg k "$KN" --arg h "$(sha256sum "$UPLOADS/$KN" | cut -d' ' -f1)" '.format == "outhouse-evidence-package/1"
  and .files[0].key == $k and .files[0].sha256 == $h and .files[0].quarantined and (.generated_by.operator | length) > 0
  and .reported_account.username == "nate" and (.blocked_networks.refused_attempts | type) == "array"' "$HD/report-$RN.json" >/dev/null
grep -q "Do not open these files" "$HD/HANDOVER.txt" && grep -q "Report:        $RN" "$HD/HANDOVER.txt" && grep -q "Australia/Sydney" "$HD/HANDOVER.txt"
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e --arg h "$PKG_SHA" 'any(.log[]; .action == "handover" and (.details | contains($h)))' >/dev/null
# The quarantined file is still never served over the web.
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$RN/files/$KN")" = 451 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN_A" "$BASE$UN")" = 451 ]
# Only child-safety reports or ones with quarantined files, unless --any; unknown reports fail.
c1=0; c2=0; c3=0
OUTHOUSE_CMD=./target/debug/outhouse-server bash deploy/evidence-handover.sh "$R1" "$HO" >/tmp/handover2.out 2>&1 || c1=$?
OUTHOUSE_CMD=./target/debug/outhouse-server bash deploy/evidence-handover.sh 12345 "$HO" >/dev/null 2>&1 || c2=$?
OUTHOUSE_CMD=./target/debug/outhouse-server bash deploy/evidence-handover.sh nope "$HO" >/dev/null 2>&1 || c3=$?
[ "$c1" = 3 ] && grep -q -- "--any" /tmp/handover2.out && [ "$c2" = 4 ] && [ "$c3" = 2 ]
OUTHOUSE_CMD=./target/debug/outhouse-server bash deploy/evidence-handover.sh --any "$R1" "$HO" >/dev/null 2>&1
# The server's evidence-cat only gives files that belong to the report.
./target/debug/outhouse-server evidence-cat "$R1" "$KN" >/dev/null 2>&1 && { echo "evidence-cat gave another report's file"; exit 1; }
rm -rf "$HO" "$HX"

step "police hand-over in the app: password + police present, one-time link (byte-identical files, 410 after), revoke, expiry, download here"
PKGDIR="$UPLOADS/.evidence-packages"
HOK='{"password":"password123","police_present":true,"reference":"Det. Sgt Smith, ref 1234/26","hours":24,"max_downloads":1}'
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/police-links" '{"password":"wrong-password","police_present":true}')" = 400 ] && grep -q "password" /tmp/body
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":false}')" = 400 ] && grep -qi "police" /tmp/body
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":true,"hours":5}')" = 400 ]
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":true,"max_downloads":4}')" = 400 ]
[ "$(tok "$TOKEN_B" status POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":true}')" = 403 ]
[ "$(tok "$TOKEN_B" status POST "/api/safety/reports/$RN/handover-download" '{"password":"password123","police_present":true}')" = 403 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$HOK" "$BASE/api/safety/reports/$RN/police-links")" = 401 ]
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/99999/police-links" "$HOK")" = 404 ]
LINK=$(tok "$TOKEN_A" call POST "/api/safety/reports/$RN/police-links" "$HOK")
URL=$(jq -r .url <<<"$LINK"); PTOK=${URL##*/}; LID=$(jq -r .link.id <<<"$LINK"); LSHA=$(jq -r .link.package_sha256 <<<"$LINK")
[[ "$URL" == "$BASE/evidence/"* ]] && [ "${#PTOK}" = 64 ] && [ "${#LSHA}" = 64 ]
jq -e --arg p "${PTOK:0:8}" '.link.status == "active" and .link.max_downloads == 1 and .link.downloads == 0 and .link.token_prefix == $p
  and .link.reference == "Det. Sgt Smith, ref 1234/26" and .link.file_count == 1 and (tostring | contains("token_sha256") | not)' <<<"$LINK" >/dev/null
# Only the token's hash is kept, and it never reaches the server log.
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM police_links WHERE token_sha256 = '$PTOK' OR token_prefix = '$PTOK'" | grep -qx 0
grep -q "$PTOK" "$LOG" && { echo "the police token is in the server log"; exit 1; }
PF=$(psql "$DATABASE_URL" -tAc "SELECT package_file FROM police_links WHERE id = $LID")
[ -f "$PKGDIR/$PF" ] && [ "$(stat -c %a "$PKGDIR/$PF")" = 600 ] && [ "$(sha256sum "$PKGDIR/$PF" | cut -d' ' -f1)" = "$LSHA" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e --arg l "$LID" --arg h "$LSHA" 'any(.police_links[]; .id == $l and .status == "active" and .package_sha256 == $h)
  and any(.log[]; .action == "handover" and (.details | contains($h)) and (.details | contains("one-time download link")) and (.details | contains("Det. Sgt Smith, ref 1234/26")))' >/dev/null
# The page police open shows the package and doesn't use the link up (nor does HEAD).
code=$(curl -sS -D /tmp/headers -o /tmp/evpage.html -w '%{http_code}' "$URL")
[ "$code" = 200 ] && grep -q "$LSHA" /tmp/evpage.html && grep -q 'method="post"' /tmp/evpage.html && grep -qi 'cache-control: no-store' /tmp/headers && grep -qi 'referrer-policy: no-referrer' /tmp/headers
grep -qi '<script' /tmp/evpage.html && { echo "the evidence page has a script"; exit 1; }
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$URL")" = 200 ] && [ "$(curl -sS -I -o /dev/null -w '%{http_code}' "$URL")" = 200 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/evidence/$(printf 'f%.0s' $(seq 64))")" = 404 ]
[ "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/evidence/not-a-token")" = 404 ]
# The download: no sign-in, a sealed tar.gz whose hash matches, files byte for byte (quarantined one too).
code=$(curl -sS -X POST -A "PoliceBrowser/1.0" -H "X-Forwarded-For: 203.0.113.77" -D /tmp/headers -o /tmp/police.tar.gz -w '%{http_code}' "$URL")
[ "$code" = 200 ] || { echo "police download -> $code"; cat /tmp/police.tar.gz; exit 1; }
grep -qi '^content-type: application/octet-stream' /tmp/headers && grep -qi '^cache-control: no-store' /tmp/headers
grep -qiE "^content-disposition: attachment; filename=\"evidence-$RN-[0-9]{8}T[0-9]{6}Z\.tar\.gz\"" /tmp/headers
[ "$(sha256sum /tmp/police.tar.gz | cut -d' ' -f1)" = "$LSHA" ]
PX=$(mktemp -d); tar -C "$PX" -xzf /tmp/police.tar.gz; PD=$(ls -d "$PX"/evidence-"$RN"-*)
( cd "$PD" && sha256sum -c --quiet MANIFEST.sha256 )
cmp "$PD/files/$KN" "$UPLOADS/$KN"
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM quarantined_files WHERE storage_key = '$KN'" | grep -qx 1
grep -q "Officer / ref: Det. Sgt Smith, ref 1234/26" "$PD/HANDOVER.txt" && grep -q "Do not open these files" "$PD/HANDOVER.txt"
grep -q "Australia/Sydney" "$PD/HANDOVER.txt" && grep -q "one-time police download link" "$PD/HANDOVER.txt" && grep -q "Report:        $RN" "$PD/HANDOVER.txt"
grep -q "files/$KN" "$PD/MANIFEST.sha256" && grep -q "report-$RN.json" "$PD/MANIFEST.sha256" && grep -q "HANDOVER.txt" "$PD/MANIFEST.sha256"
jq -e --arg k "$KN" '.files[0].key == $k and .files[0].quarantined and (.files[0].detector | type) == "object"
  and .generated_by.route == "link" and .generated_by.reference == "Det. Sgt Smith, ref 1234/26"
  and (.files_note | test("in this hand-over package")) and .reported_account.username == "nate" and (.reported_account | has("suspended"))
  and (.ip_addresses.addresses | type) == "array" and (.blocked_networks.refused_attempts | type) == "array"
  and any(.safety_log[]; .action == "quarantine") and (.reporters | length) >= 1' "$PD/report-$RN.json" >/dev/null
# Used up: 410 (page and download), and the package is gone from the server.
[ "$(curl -sS -X POST -o /tmp/body -w '%{http_code}' "$URL")" = 410 ]
[ "$(curl -sS -o /tmp/body -w '%{http_code}' "$URL")" = 410 ] && grep -q "already" /tmp/body
[ ! -e "$PKGDIR/$PF" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e --arg l "$LID" --arg h "$LSHA" 'any(.police_links[]; .id == $l and .status == "used" and .downloads == 1 and .package_deleted)
  and any(.log[]; .action == "handover_download" and (.details | contains("203.0.113.77")) and (.details | contains("PoliceBrowser/1.0"))
    and (.details | contains($h)) and (.details | contains("1 of 1")))' >/dev/null
# Two downloads allowed, then it expires: 410 and the package is deleted.
L2=$(tok "$TOKEN_A" call POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":true,"hours":1,"max_downloads":2}')
U2=$(jq -r .url <<<"$L2"); L2ID=$(jq -r .link.id <<<"$L2"); PF2=$(psql "$DATABASE_URL" -tAc "SELECT package_file FROM police_links WHERE id = $L2ID")
[ "$(curl -sS -X POST -o /tmp/p2.tar.gz -w '%{http_code}' "$U2")" = 200 ] && [ "$(sha256sum /tmp/p2.tar.gz | cut -d' ' -f1)" = "$(jq -r .link.package_sha256 <<<"$L2")" ]
[ -f "$PKGDIR/$PF2" ] && grep -q "1 of 2" <(curl -sS "$U2")
psql "$DATABASE_URL" -qc "UPDATE police_links SET expires_at = now() - interval '1 minute' WHERE id = $L2ID"
[ "$(curl -sS -X POST -o /tmp/body -w '%{http_code}' "$U2")" = 410 ] && grep -q "expired" /tmp/body
[ ! -e "$PKGDIR/$PF2" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e --arg l "$L2ID" 'any(.police_links[]; .id == $l and .status == "expired" and .downloads == 1 and .package_deleted)' >/dev/null
# Revoke: stops at once and deletes the package (owner only).
L3=$(tok "$TOKEN_A" call POST "/api/safety/reports/$RN/police-links" '{"password":"password123","police_present":true,"hours":168,"max_downloads":3}')
U3=$(jq -r .url <<<"$L3"); L3ID=$(jq -r .link.id <<<"$L3"); PF3=$(psql "$DATABASE_URL" -tAc "SELECT package_file FROM police_links WHERE id = $L3ID")
[ -f "$PKGDIR/$PF3" ]
tok "$TOKEN_B" fail_call POST "/api/safety/reports/$RN/police-links/$L3ID/revoke"
tok "$TOKEN_A" call POST "/api/safety/reports/$RN/police-links/$L3ID/revoke" | jq -e --arg l "$L3ID" 'any(.links[]; .id == $l and .status == "revoked" and .package_deleted)' >/dev/null
[ ! -e "$PKGDIR/$PF3" ] && [ "$(curl -sS -X POST -o /tmp/body -w '%{http_code}' "$U3")" = 410 ] && grep -q "withdrawn" /tmp/body
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/police-links/$L3ID/revoke")" = 409 ]
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e 'any(.log[]; .action == "handover_revoke")' >/dev/null
# Download here (only if police ask): the same package straight to the browser, logged, nothing left on the server.
[ "$(tok "$TOKEN_A" status POST "/api/safety/reports/$RN/handover-download" '{"password":"nope","police_present":true}')" = 400 ]
code=$(curl -sS -X POST -H "Authorization: Bearer $TOKEN_A" -H 'Content-Type: application/json' -d '{"password":"password123","police_present":true,"reference":"Const. Jones"}' \
  -D /tmp/headers -o /tmp/here.tar.gz -w '%{http_code}' "$BASE/api/safety/reports/$RN/handover-download")
[ "$code" = 200 ] && grep -qi '^content-type: application/octet-stream' /tmp/headers && grep -qi '^cache-control: no-store' /tmp/headers
HSHA=$(grep -i '^x-package-sha256:' /tmp/headers | tr -d '\r' | awk '{print $2}')
[ "$(sha256sum /tmp/here.tar.gz | cut -d' ' -f1)" = "$HSHA" ]
HX2=$(mktemp -d); tar -C "$HX2" -xzf /tmp/here.tar.gz; HD2=$(ls -d "$HX2"/evidence-"$RN"-*)
( cd "$HD2" && sha256sum -c --quiet MANIFEST.sha256 ) && cmp "$HD2/files/$KN" "$UPLOADS/$KN" && grep -q "Const. Jones" "$HD2/HANDOVER.txt"
tok "$TOKEN_A" call GET "/api/safety/reports/$RN" | jq -e --arg h "$HSHA" 'any(.log[]; .action == "handover" and (.details | contains($h)) and (.details | contains("downloaded here")) and (.details | contains("Const. Jones")))' >/dev/null
[ -z "$(find "$PKGDIR" -type f -newer /tmp/here.tar.gz 2>/dev/null)" ]
[ "$(find "$PKGDIR" -type f | wc -l)" = 0 ] || { echo "packages left on the server:"; ls -la "$PKGDIR"; exit 1; }
# A report without files: the JSON, HANDOVER.txt and the manifest still.
code=$(curl -sS -X POST -H "Authorization: Bearer $TOKEN_A" -H 'Content-Type: application/json' -d '{"password":"password123","police_present":true}' \
  -o /tmp/nofiles.tar.gz -w '%{http_code}' "$BASE/api/safety/reports/$R5/handover-download")
[ "$code" = 200 ]
NX=$(mktemp -d); tar -C "$NX" -xzf /tmp/nofiles.tar.gz; ND=$(ls -d "$NX"/evidence-"$R5"-*)
( cd "$ND" && sha256sum -c --quiet MANIFEST.sha256 ) && [ -f "$ND/report-$R5.json" ] && [ -z "$(ls -A "$ND/files")" ] && grep -q "Files:         0" "$ND/HANDOVER.txt"
# Guessing at links is rate limited (per address).
for _ in $(seq 1 30); do curl -sS -o /dev/null -H "X-Forwarded-For: 198.51.100.9" "$BASE/evidence/$(printf '0%.0s' $(seq 64))"; done
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "X-Forwarded-For: 198.51.100.9" "$BASE/evidence/$(printf '0%.0s' $(seq 64))")" = 429 ]
grep -q "$PTOK" "$LOG" && { echo "the police token is in the server log"; exit 1; }
rm -rf "$PX" "$HX2" "$NX" /tmp/police.tar.gz /tmp/here.tar.gz /tmp/p2.tar.gz /tmp/nofiles.tar.gz

# ---------------------------------------------------------------------------
# IP addresses (feat-ips): kept for safety, site owner only, same-IP hints
# ---------------------------------------------------------------------------
# The smoke test talks to the server from 127.0.0.1, a trusted proxy, so the
# X-Forwarded-For here stands in for Apache/Caddy's.
ipreq() { # ipreq XFF PATH JSON [TOKEN] -> HTTP status (body in /tmp/body)
  local args=(-sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: $1")
  [ -n "${4:-}" ] && args+=(-H "Authorization: Bearer $4")
  if [ -n "$3" ]; then curl "${args[@]}" -d "$3" "$BASE$2"; else curl "${args[@]}" "$BASE$2"; fi
}
uip() { # uip USER IP -> "kind,count" (empty if not recorded)
  psql "$DATABASE_URL" -tAc "SELECT kind || ',' || count FROM user_ips WHERE user_id = $1 AND ip = '$2'"
}
ipm_count() { psql "$DATABASE_URL" -tAc "SELECT count(*) FROM reports WHERE source = 'ip-match' AND reported_user_id = $1"; }
purge_ips() { tok "$TOKEN_A" call POST /api/safety/ips/purge; }

step "IPs: sign-up and sign-in record the visitor's address from the proxy chain"
# Caddy sends "<visitor>, <Apache>": the visitor is recorded, not Apache.
[ "$(ipreq '203.0.113.20, 192.168.1.32' /api/auth/register '{"username":"ivy","email":"ivy@example.test","password":"password123","display_name":"Ivy","birth_date":"1990-06-06"}')" = 200 ]
TOKEN_IVY=$(jq -r .token /tmp/body); IVY=$(jq -r .user.id /tmp/body)
[ "$(uip "$IVY" 203.0.113.20)" = "register,1" ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT host(last_ip) FROM users WHERE id = $IVY")" = 203.0.113.20 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM user_ips WHERE user_id = $IVY AND ip = '192.168.1.32'")" = 0 ]
[ "$(ipreq '198.51.100.20, 192.168.1.32' /api/auth/login '{"username":"ivy","password":"password123"}')" = 200 ]
[ "$(uip "$IVY" 198.51.100.20)" = "login,1" ]
# The same address again counts up (one row per account and address).
[ "$(ipreq '203.0.113.20, 192.168.1.32' /api/auth/login '{"username":"ivy","password":"password123"}')" = 200 ]
TOKEN_IVY2=$(jq -r .token /tmp/body)
[ "$(uip "$IVY" 203.0.113.20)" = "register,2" ]

step "IPs: signed-in activity is noted at most once an hour per session (and the gateway too); bots never"
[ "$(ipreq '203.0.113.20' /api/users/@me '' "$TOKEN_IVY2")" = 200 ]   # sign-in already counted it
[ "$(ipreq '198.51.100.99' /api/users/@me '' "$TOKEN_IVY2")" = 200 ]
[ "$(ipreq '198.51.100.99' /api/users/@me '' "$TOKEN_IVY2")" = 200 ]
for _ in $(seq 1 20); do [ -n "$(uip "$IVY" 198.51.100.99)" ] && break; sleep 0.25; done
sleep 0.5
[ "$(uip "$IVY" 198.51.100.99)" = "session,1" ]
[ "$(uip "$IVY" 203.0.113.20)" = "register,2" ]
node ci/gateway_probe.mjs "$BASE" "$TOKEN_IVY" /tmp/gw-ivy.jsonl 5 & GWI=$!
wait_for /tmp/gw-ivy.jsonl '.t == "READY"'
for _ in $(seq 1 20); do [ -n "$(uip "$IVY" 127.0.0.1)" ] && break; sleep 0.25; done
[ "$(uip "$IVY" 127.0.0.1)" = "session,1" ]
wait $GWI 2>/dev/null || true
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM user_ips i JOIN users u ON u.id = i.user_id WHERE u.is_bot")" = 0 ]

step "IPs: nobody but the site owner can see any address, not even their own"
for t in "$TOKEN_B" "$TOKEN_IVY"; do
  tok "$t" fail_call GET "/api/safety/users/$IVY/ips"
  tok "$t" fail_call POST /api/safety/ips/purge
done
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/api/safety/users/$IVY/ips")" = 401 ]
for path in /api/users/@me "/api/profiles/$IVY" /api/users/@me/privacy; do
  tok "$TOKEN_IVY" call GET "$path" | grep -qE '203\.0\.113|198\.51\.100|last_ip' && { echo "an address leaked from $path"; exit 1; }
done
tok "$TOKEN_B" call GET "/api/profiles/$IVY" | grep -qE '203\.0\.113|198\.51\.100|last_ip' && { echo "an address leaked to another user"; exit 1; }
curl -fsS "$BASE/api/auth/registration" | jq -e '.ip_retention_days == 90' >/dev/null

step "IPs: the owner sees someone's addresses and who else used them, and each look is logged"
tok "$TOKEN_A" call GET "/api/safety/users/$IVY/ips" | jq -e '
  (.ips | length) == 4 and .retention_days == 90 and (.note | test("kept 90 days") and test("hint, not proof"))
  and any(.ips[]; .ip == "203.0.113.20" and .kind == "register" and .count == 2 and (.local | not))
  and any(.ips[]; .ip == "198.51.100.20" and .kind == "login")
  and any(.ips[]; .ip == "127.0.0.1" and .local)' >/dev/null
tok "$TOKEN_A" call GET /api/safety/log | jq -e --arg u "$IVY" '.entries[0].action == "view_ips" and .entries[0].target_user_id == $u and (.entries[0].details | test("@ivy"))' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$IVY" | jq -e '.ip_note | test("Shared IPs are common")' >/dev/null

step "IPs: a new account on a suspended or banned account's address gets one quiet hint for the owner"
OPEN_BEFORE=$(tok "$TOKEN_A" call GET /api/safety/summary | jq .open)
NTFY_BEFORE=$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)
[ "$(ipreq '198.18.20.1' /api/auth/register '{"username":"zed","email":"zed@example.test","password":"password123","display_name":"Zed","birth_date":"1990-07-07"}')" = 200 ]
ZED=$(jq -r .user.id /tmp/body)
TOKEN="" call POST /api/auth/login '{"username":"zed","password":"password123"}' >/dev/null   # also from 127.0.0.1
tok "$TOKEN_A" call POST "/api/safety/users/$ZED/actions" '{"action":"suspend","days":7,"text":"ban evasion test"}' >/dev/null
[ "$(ipreq '198.18.20.1, 192.168.1.32' /api/auth/register '{"username":"jax","email":"jax@example.test","password":"password123","display_name":"Jax","birth_date":"1990-08-08"}')" = 200 ]
JAX=$(jq -r .user.id /tmp/body)
[ "$(ipm_count "$JAX")" = 1 ]
RIPM=$(psql "$DATABASE_URL" -tAc "SELECT id FROM reports WHERE source = 'ip-match' AND reported_user_id = $JAX")
# Quiet: not in the open count (no badge), no push or alert, but counted on its own.
tok "$TOKEN_A" call GET /api/safety/summary | jq -e --argjson o "$OPEN_BEFORE" '.open == $o and .ip_matches >= 1' >/dev/null
[ "$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)" = "$NTFY_BEFORE" ]
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg r "$RIPM" --arg j "$JAX" '
  .counts.ip_matches >= 1 and (.reports[-1].priority == 3)
  and any(.reports[]; .id == $r and .source == "ip-match" and .reported_user.id == $j and (.preview | test("Signed up") and test("@zed") and test("suspended")))' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$RIPM" | jq -e --arg z "$ZED" --arg j "$JAX" '
  .snapshot.kind == "ip-match" and .snapshot.account.id == $j and .snapshot.matched.id == $z and .snapshot.matched.suspended
  and (.snapshot | tostring | test("198\\.18\\.20\\.1") | not) and .reporters == []' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$JAX/ips" | jq -e --arg z "$ZED" 'any(.same_ip_accounts[]; .id == $z and .suspended and .flagged and (.shared_ips | index("198.18.20.1")))' >/dev/null
tok "$TOKEN_B" fail_call GET "/api/safety/reports/$RIPM"
# Never repeated for the same pair (signing in again from there).
[ "$(ipreq '198.18.20.1' /api/auth/login '{"username":"jax","password":"password123"}')" = 200 ]
[ "$(ipm_count "$JAX")" = 1 ]
# Banned from an outhouse counts too (sign-in this time).
tok "$TOKEN_IVY" call POST "/api/invites/$SINV2" >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$IVY/actions" '{"action":"ban","guild_id":"'"$GUILD"'","text":"ip test"}' >/dev/null
[ "$(ipreq '198.51.100.20' /api/auth/login '{"username":"jax","password":"password123"}')" = 200 ]
[ "$(ipm_count "$JAX")" = 2 ]
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg j "$JAX" 'any(.reports[]; .source == "ip-match" and .reported_user.id == $j and (.preview | test("Signed in") and test("@ivy") and test("banned in Smoke Test")))' >/dev/null
# Home-network (LAN) and trusted-proxy addresses never make a hint, and nobody matches themselves.
[ "$(ipreq '192.168.1.77' /api/auth/register '{"username":"lana","email":"lana@example.test","password":"password123","birth_date":"1990-09-09"}')" = 200 ]
LANA=$(jq -r .user.id /tmp/body)
tok "$TOKEN_A" call POST "/api/safety/users/$LANA/actions" '{"action":"suspend","days":1}' >/dev/null
[ "$(ipreq '192.168.1.77' /api/auth/register '{"username":"lena","email":"lena@example.test","password":"password123","birth_date":"1990-09-09"}')" = 200 ]
LENA=$(jq -r .user.id /tmp/body)
[ "$(uip "$LENA" 192.168.1.77)" = "register,1" ]
[ "$(ipm_count "$LENA")" = 0 ]
TOKEN="" call POST /api/auth/login '{"username":"ivy","password":"password123"}' >/dev/null    # 127.0.0.1, which Zed used too
[ "$(ipm_count "$IVY")" = 0 ]
tok "$TOKEN_A" call GET /api/safety/digest | jq -e '.ip_matches >= 2' >/dev/null

step "IPs: the evidence package has the reported person's addresses in UTC and Sydney time"
[ "$(report "$TOKEN_B" user "$IVY" harassment)" = 201 ]; RIVY=$(report_id user "$IVY")
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM report_ips WHERE report_id = $RIVY AND user_id = $IVY")" = 4 ]
curl -fsS -H "Authorization: Bearer $TOKEN_A" "$BASE/api/safety/reports/$RIVY/export" -o /tmp/export-ivy.json
jq -e --arg i "$IVY" '(.ip_addresses.note | test("UTC") and test("Australia/Sydney"))
  and any(.ip_addresses.addresses[]; .account_id == $i and .ip_address == "203.0.113.20" and .first_seen_as == "register" and .times_seen >= 2
      and (.first_seen_utc | test("^[0-9-]{10}T[0-9:]{8}Z$"))
      and (.last_seen_australia_sydney | test("^[0-9-]{10} [0-9:]{8} AE[SD]T \\(UTC\\+1[01]\\)$")))' /tmp/export-ivy.json >/dev/null

step "IPs: retention deletes addresses not seen for 90 days; an open report keeps its copy"
psql "$DATABASE_URL" -qc "UPDATE user_ips SET first_seen = now() - interval '120 days', last_seen = now() - interval '100 days'
  WHERE (user_id = $IVY AND ip = '198.51.100.20') OR user_id = $LENA;
  UPDATE users SET last_ip_at = now() - interval '100 days' WHERE id = $LENA"
purge_ips | jq -e '.addresses >= 2 and .kept_for_reports >= 1' >/dev/null
[ -z "$(uip "$IVY" 198.51.100.20)" ] && [ -z "$(uip "$LENA" 192.168.1.77)" ]
[ -n "$(uip "$IVY" 203.0.113.20)" ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT last_ip IS NULL FROM users WHERE id = $LENA")" = t ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM report_ips WHERE report_id = $RIVY AND ip = '198.51.100.20'")" = 1 ]
tok "$TOKEN_A" call GET "/api/safety/users/$IVY/ips" | jq -e 'all(.ips[]; .ip != "198.51.100.20")' >/dev/null
curl -fsS -H "Authorization: Bearer $TOKEN_A" "$BASE/api/safety/reports/$RIVY/export" \
  | jq -e 'any(.ip_addresses.addresses[]; .ip_address == "198.51.100.20")' >/dev/null
# Closed: the copy goes once the retention period has passed since closing.
act "$RIVY" '{"action":"close","text":"ip test"}' >/dev/null
purge_ips >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM report_ips WHERE report_id = $RIVY")" -ge 1 ]
psql "$DATABASE_URL" -qc "UPDATE reports SET closed_at = now() - interval '91 days' WHERE id = $RIVY"
purge_ips | jq -e '.report_addresses >= 1' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM report_ips WHERE report_id = $RIVY")" = 0 ]
# Tidy up for the sections below.
tok "$TOKEN_A" call DELETE "/api/guilds/$GUILD/bans/$IVY" >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$ZED/actions" '{"action":"unsuspend"}' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$LANA/actions" '{"action":"unsuspend"}' >/dev/null

step "blocked networks: only for permanent suspensions; v4 /32 and v6 /64; private, own and proxy addresses skipped"
reload_blocks() { tok "$TOKEN_A" call POST /api/safety/ip-blocks/reload >/dev/null; }
[ "$(ipreq '203.0.113.150' /api/auth/register '{"username":"pete","email":"pete@example.test","password":"password123","display_name":"Pete","birth_date":"1990-08-08"}')" = 200 ]
TOKEN_PE=$(jq -r .token /tmp/body); PETE=$(jq -r .user.id /tmp/body)
[ "$(ipreq '2001:db8:abcd:12::5' /api/auth/login '{"username":"pete","password":"password123"}')" = 200 ]
[ "$(ipreq '10.0.0.9' /api/auth/login '{"username":"pete","password":"password123"}')" = 200 ]
[ "$(ipreq '100.64.3.3' /api/auth/login '{"username":"pete","password":"password123"}')" = 200 ]
[ "$(ipreq '203.0.113.151' /api/auth/login '{"username":"pete","password":"password123"}')" = 200 ]
# The site owner uses 203.0.113.151 too (their household).
[ "$(ipreq '203.0.113.151' /api/users/@me '' "$TOKEN_A")" = 200 ]
psql "$DATABASE_URL" -qc "INSERT INTO user_ips (user_id, ip, kind) VALUES ($ALICE, '203.0.113.151', 'login') ON CONFLICT DO NOTHING"
# Quinn already used Pete's network before the block (a housemate).
[ "$(ipreq '203.0.113.150' /api/auth/register '{"username":"quinn","email":"quinn@example.test","password":"password123","display_name":"Quinn","birth_date":"1991-09-09"}')" = 200 ]
TOKEN_Q=$(jq -r .token /tmp/body); QUINN=$(jq -r .user.id /tmp/body)
tok "$TOKEN_PE" call POST "/api/invites/$SINV2" >/dev/null
PM=$(tok "$TOKEN_PE" call POST "/api/channels/$CHAN/messages" '{"content":"pete was here"}' | jq -r .id)
[ "$(report "$TOKEN_D" message "$PM" child_safety)" = 201 ]; RPE=$(report_id message "$PM")
# Not permanent yet: no blocks.
act "$RPE" '{"action":"suspend_pending"}' >/dev/null
tok "$TOKEN_A" fail_call POST "/api/safety/users/$PETE/ip-blocks" '{}'
tok "$TOKEN_B" fail_call GET "/api/safety/users/$PETE/ip-block-preview"
tok "$TOKEN_B" fail_call GET /api/safety/ip-blocks
act "$RPE" '{"action":"make_permanent","text":"Grooming"}' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$PETE/ip-block-preview" | jq -e '.child_safety and .permanent
  and any(.networks[]; .network == "203.0.113.150/32" and .skip == null and (.own | not))
  and any(.networks[]; .network == "2001:db8:abcd:12::/64" and .skip == null)
  and any(.networks[]; .skip != null and (.skip | test("private")))
  and any(.networks[]; .network == "100.64.3.3/32" and (.skip | test("carrier-grade")))
  and any(.networks[]; .network == "203.0.113.151/32" and .own)
  and (.note | test("locked out"))' >/dev/null
node ci/gateway_probe.mjs "$BASE" "$TOKEN_Q" /tmp/gw-quinn.jsonl 60 & SPQ=$!
wait_for /tmp/gw-quinn.jsonl '.t == "READY"'
tok "$TOKEN_A" call POST "/api/safety/users/$PETE/ip-blocks" '{"reason":"child safety"}' > /tmp/blocks.json
jq -e '(.blocked | sort) == ["2001:db8:abcd:12::/64","203.0.113.150/32"] and .signed_out >= 1
  and any(.skipped[]; test("203.0.113.151/32 \\(one of your own")) and any(.skipped[]; test("10.0.0.9"))' /tmp/blocks.json >/dev/null
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM ip_blocks WHERE source_user_id = $PETE AND report_id = $RPE AND expires_at IS NULL AND removed_at IS NULL" | grep -qx 2
# Everyone seen on a blocked network is signed out at once (Quinn, the housemate).
wait_for /tmp/gw-quinn.jsonl '.t == "INVALID_SESSION" and (.d.reason | test("your network"))'
kill $SPQ 2>/dev/null || true
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN_Q" "$BASE/api/users/@me")" = 401 ]

step "blocked networks: strict: no sign-up, sign-in or use from them (any account), the owner and allowed accounts excepted; attempts kept"
ATT0=$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts")
[ "$(ipreq '203.0.113.150' /api/auth/register '{"username":"pete2","email":"pete2@example.test","password":"password123","birth_date":"1990-08-08"}')" = 403 ]
jq -e '.error == "New accounts can'"'"'t be created from your network right now. If you think this is a mistake, contact the server owner."' /tmp/body >/dev/null
[ "$(ipreq '2001:db8:abcd:12:ffff::1' /api/auth/register '{"username":"pete3","email":"pete3@example.test","password":"password123","birth_date":"1990-08-08"}')" = 403 ]   # same /64
[ "$(ipreq '2001:db8:abcd:13::1' /api/auth/register '{"username":"olive","email":"olive@example.test","password":"password123","birth_date":"1990-08-08"}')" = 200 ]  # next /64: fine
[ "$(ipreq '203.0.113.150' /api/auth/login '{"username":"quinn","password":"password123"}')" = 403 ]
jq -e '.error | test("can'"'"'t be used from your network")' /tmp/body >/dev/null
[ "$(ipreq '203.0.113.150' /api/auth/login '{"username":"quinn","password":"wrong-password"}')" = 403 ]   # says nothing about the password
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM users WHERE username IN ('pete2','pete3')" | grep -qx 0
# Signed in elsewhere, then used from the blocked network: refused and signed out.
[ "$(ipreq '198.51.100.150' /api/auth/login '{"username":"quinn","password":"password123"}')" = 200 ]
TOKEN_Q=$(jq -r .token /tmp/body)
[ "$(ipreq '198.51.100.150' /api/users/@me '' "$TOKEN_Q")" = 200 ]
[ "$(ipreq '203.0.113.150' /api/users/@me '' "$TOKEN_Q")" = 403 ]
[ "$(ipreq '198.51.100.150' /api/users/@me '' "$TOKEN_Q")" = 401 ]
# The site owner always gets through.
[ "$(ipreq '203.0.113.150' /api/users/@me '' "$TOKEN_A")" = 200 ]
# "Allow from blocked networks" for someone the owner knows is innocent.
tok "$TOKEN_A" call POST "/api/safety/users/$QUINN/actions" '{"action":"set_ip_exempt","exempt":true}' | jq -e '.ip_block_exempt' >/dev/null
[ "$(ipreq '203.0.113.150' /api/auth/login '{"username":"quinn","password":"password123"}')" = 200 ]
TOKEN_Q=$(jq -r .token /tmp/body)
[ "$(ipreq '203.0.113.150' /api/users/@me '' "$TOKEN_Q")" = 200 ]
tok "$TOKEN_A" call POST "/api/safety/users/$QUINN/actions" '{"action":"set_ip_exempt","exempt":false}' | jq -e '(.ip_block_exempt | not) and any(.log[]; .action == "ip_exempt")' >/dev/null
[ "$(ipreq '203.0.113.150' /api/users/@me '' "$TOKEN_Q")" = 403 ]
# Every refusal is written down (time, address, account or username tried).
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts")" -ge $((ATT0 + 5)) ]
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts WHERE kind = 'register' AND username = 'pete2' AND ip = '203.0.113.150'" | grep -qx 1
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts WHERE kind = 'login' AND user_id = $QUINN" | grep -qxE '[2-9]'
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts WHERE kind = 'request' AND user_id = $QUINN" | grep -qxE '[1-9]'
tok "$TOKEN_A" call GET "/api/safety/users/$PETE" | jq -e '.ip_blocks == 2 and any(.blocked_attempts[]; .username == "pete2" and .kind == "register")' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$QUINN" | jq -e 'any(.blocked_attempts[]; .kind == "login")' >/dev/null
tok "$TOKEN_A" call GET /api/safety/ip-blocks | jq -e '[.blocks[] | select(.network == "203.0.113.150/32")][0] | .attempts >= 4 and .account.username == "pete" and .active and .expires_at == null' >/dev/null
# ...and goes in the report's evidence package.
tok "$TOKEN_A" call GET "/api/safety/reports/$RPE/export" | jq -e '(.blocked_networks.blocks | length) == 2
  and any(.blocked_networks.refused_attempts[]; .username_tried == "pete2" and .attempt == "register" and .ip_address == "203.0.113.150"
          and (.time_australia_sydney | test("AE[SD]T")) and (.time_utc | endswith("Z")))' >/dev/null

step "blocked networks: expiry, extend, remove (the record stays), safety log"
B4=$(tok "$TOKEN_A" call GET /api/safety/ip-blocks | jq -r '.blocks[] | select(.network == "203.0.113.150/32") | .id')
B6=$(tok "$TOKEN_A" call GET /api/safety/ip-blocks | jq -r '.blocks[] | select(.network == "2001:db8:abcd:12::/64") | .id')
tok "$TOKEN_A" call POST "/api/safety/ip-blocks/$B4/extend" '{"days":365}' >/dev/null
psql "$DATABASE_URL" -tAc "SELECT (expires_at > now() + interval '364 days')::TEXT FROM ip_blocks WHERE id = $B4" | grep -qx true
psql "$DATABASE_URL" -qc "UPDATE ip_blocks SET expires_at = now() - interval '1 second' WHERE id = $B4"
reload_blocks
[ "$(ipreq '203.0.113.150' /api/auth/register '{"username":"pete4","email":"pete4@example.test","password":"password123","birth_date":"1990-08-08"}')" = 200 ]
tok "$TOKEN_A" call GET /api/safety/ip-blocks | jq -e --arg b "$B4" '.blocks[] | select(.id == $b) | .active | not' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/ip-blocks/$B4/extend" '{}' >/dev/null    # until removed again
[ "$(ipreq '203.0.113.150' /api/auth/register '{"username":"pete5","email":"pete5@example.test","password":"password123","birth_date":"1990-08-08"}')" = 403 ]
tok "$TOKEN_A" call POST "/api/safety/ip-blocks/$B6/remove" >/dev/null
[ "$(ipreq '2001:db8:abcd:12::77' /api/auth/register '{"username":"pete6","email":"pete6@example.test","password":"password123","birth_date":"1990-08-08"}')" = 200 ]
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM ip_blocks WHERE id = $B6 AND removed_at IS NOT NULL" | grep -qx 1
tok "$TOKEN_A" call GET /api/safety/ip-blocks | jq -e --arg b "$B6" 'all(.blocks[]; .id != $b)' >/dev/null
tok "$TOKEN_A" call GET /api/safety/log | jq -e '[.entries[].action] | (index("ip_block") and index("ip_block_extend") and index("ip_unblock") and index("ip_exempt"))' >/dev/null
tok "$TOKEN_B" fail_call POST "/api/safety/ip-blocks/$B4/remove"
# Tidy up for the sections below.
tok "$TOKEN_A" call POST "/api/safety/ip-blocks/$B4/remove" >/dev/null
[ "$(ipreq '203.0.113.150' /api/auth/login '{"username":"quinn","password":"password123"}')" = 200 ]

step "Deleting a channel's newest message moves its newest back, so it can't stay unread"
UC=$(tok "$TOKEN_A" call POST "/api/guilds/$GUILD/channels" '{"name":"unread-fix"}' | jq -r .id)
U1=$(tok "$TOKEN_A" call POST "/api/channels/$UC/messages" '{"content":"first"}' | jq -r .id)
U2=$(tok "$TOKEN_A" call POST "/api/channels/$UC/messages" '{"content":"second"}' | jq -r .id)
tok "$TOKEN_B" call GET "/api/users/@me/unread" | jq -e --arg c "$UC" --arg m "$U2" '.channels[] | select(.channel_id == $c) | .last_message_id == $m' >/dev/null
tok "$TOKEN_A" call DELETE "/api/channels/$UC/messages/$U2" >/dev/null
tok "$TOKEN_B" call GET "/api/users/@me/unread" | jq -e --arg c "$UC" --arg m "$U1" '.channels[] | select(.channel_id == $c) | .last_message_id == $m' >/dev/null
tok "$TOKEN_A" call DELETE "/api/channels/$UC/messages/$U1" >/dev/null
tok "$TOKEN_B" call GET "/api/users/@me/unread" | jq -e --arg c "$UC" '.channels[] | select(.channel_id == $c) | .last_message_id == null' >/dev/null

# ---------------------------------------------------------------------------
# Full profiles, name history and DM message requests (feat-profiles)
# ---------------------------------------------------------------------------
step "profiles: name history (display names, nicknames) is recorded and shown by who can see it"
[ "$(xreg 11 '{"username":"pia","email":"pia@example.test","password":"password123","display_name":"Pia","birth_date":"1991-03-03"}')" = 200 ]
TOKEN_P=$(jq -r .token /tmp/body); PIA=$(jq -r .user.id /tmp/body)
[ "$(xreg 12 '{"username":"quin","email":"quin@example.test","password":"password123","display_name":"Quin","birth_date":"1992-04-04"}')" = 200 ]
TOKEN_Q=$(jq -r .token /tmp/body); QUIN=$(jq -r .user.id /tmp/body)
[ "$(xreg 13 '{"username":"rex","email":"rex@example.test","password":"password123","display_name":"Rex","birth_date":"1993-05-05"}')" = 200 ]
TOKEN_R=$(jq -r .token /tmp/body); REX=$(jq -r .user.id /tmp/body)
# Pia and Quin share "Profile Porch"; Rex shares nothing with anyone.
PG=$(tok "$TOKEN_P" call POST /api/guilds '{"name":"Profile Porch"}' | jq -r .id)
PINV=$(tok "$TOKEN_P" call POST "/api/guilds/$PG/invites" | jq -r .code)
tok "$TOKEN_Q" call POST "/api/invites/$PINV" >/dev/null
# No earlier names yet.
tok "$TOKEN_Q" call GET "/api/users/$PIA/profile/full" | jq -e '.level == "full" and .names == []' >/dev/null
tok "$TOKEN_P" call PATCH /api/users/@me '{"display_name":"Pia Two"}' >/dev/null
tok "$TOKEN_P" call PATCH /api/users/@me '{"display_name":"Pia Three"}' >/dev/null
tok "$TOKEN_P" call PUT "/api/guilds/$PG/nickname" '{"nickname":"Porch Pia"}' >/dev/null
# A nickname in an outhouse Quin isn't in.
PG2=$(tok "$TOKEN_P" call POST /api/guilds '{"name":"Secret Shed"}' | jq -r .id)
tok "$TOKEN_P" call PUT "/api/guilds/$PG2/nickname" '{"nickname":"Shed Pia"}' >/dev/null
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM name_history WHERE user_id = $PIA" | grep -qx 4
# Quin (shares an outhouse): display names and the shared outhouse's nickname, newest first.
tok "$TOKEN_Q" call GET "/api/users/$PIA/profile/full" | jq -e --arg g "$PG" '.level == "full" and (.names | length == 3)
  and .names[0].kind == "nickname" and .names[0].new == "Porch Pia" and .names[0].guild_id == $g
  and .names[1].old == "Pia Two" and .names[1].new == "Pia Three" and .names[2].old == "Pia"
  and all(.names[]; .new != "Shed Pia") and (.outhouses | length == 1) and .outhouses[0].nickname == "Porch Pia"
  and .outhouses[0].owner and (.outhouses[0].joined_at | length > 0) and (.contact.request | not) and .contact.can_message' >/dev/null
# Pia and the site owner see all of it; the owner's console too.
tok "$TOKEN_P" call GET "/api/users/$PIA/profile/full" | jq -e '.self and (.names | length == 4) and .contact == null' >/dev/null
tok "$TOKEN_A" call GET "/api/users/$PIA/profile/full" | jq -e '.site_owner_view and .level == "full" and (.names | length == 4)' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$PIA" | jq -e '(.names | length) == 4 and .message_requests.sent == 0' >/dev/null
# Nobody can delete it (there's no way to), and someone with no link to Pia can't look at all.
tok "$TOKEN_P" fail_call DELETE "/api/users/$PIA/profile/full"
[ "$(tok "$TOKEN_R" status GET "/api/users/$PIA/profile/full")" = 404 ]
# At most 20 shown; every change is kept.
for i in $(seq 1 22); do tok "$TOKEN_Q" call PATCH /api/users/@me "{\"display_name\":\"Quin $i\"}" >/dev/null; done
tok "$TOKEN_P" call GET "/api/users/$QUIN/profile/full" | jq -e '(.names | length) == 20 and .names[0].new == "Quin 22"' >/dev/null
psql "$DATABASE_URL" -tAc "SELECT count(*) FROM name_history WHERE user_id = $QUIN" | grep -qx 22
tok "$TOKEN_Q" call PATCH /api/users/@me '{"display_name":"Quin"}' >/dev/null

step "message requests: a stranger's DM is held (no events, unread or pictures) until accepted"
tok "$TOKEN_R" call GET "/api/users/$PIA/contact" | jq -e '.can_message and .request' >/dev/null
tok "$TOKEN_Q" call GET "/api/users/$PIA/contact" | jq -e '.can_message and (.request | not)' >/dev/null
node ci/gateway_probe.mjs "$BASE" "$TOKEN_P" /tmp/gw-req-pia.jsonl 40 &
wait_for /tmp/gw-req-pia.jsonl '.t == "READY"'
RDM=$(tok "$TOKEN_R" call POST /api/dms '{"user_ids":["'"$PIA"'"]}')
jq -e --arg p "$PIA" '.request == "outgoing" and any(.recipients[]; .id == $p)' <<<"$RDM" >/dev/null
RDMID=$(jq -r .id <<<"$RDM")
tok "$TOKEN_R" call POST /api/dms '{"user_ids":["'"$PIA"'"]}' | jq -e --arg id "$RDMID" '.id == $id' >/dev/null # same one again
# Nothing sent yet: nothing to show her.
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
tok "$TOKEN_R" call POST "/api/channels/$RDMID/messages" '{"content":"hi <@'"$PIA"'>, saw your raid post"}' >/dev/null
fupload "$TOKEN_R" "$RDMID" "a picture"
tok "$TOKEN_P" call GET /api/dms | jq -e --arg id "$RDMID" 'all(.[]; .id != $id)' >/dev/null
tok "$TOKEN_P" call GET /api/users/@me/unread | jq -e --arg id "$RDMID" 'all(.channels[]; .channel_id != $id)' >/dev/null
tok "$TOKEN_P" fail_call GET "/api/channels/$RDMID/messages"
tok "$TOKEN_P" call GET /api/dms/requests | jq -e --arg r "$REX" --arg c "$RDMID" '.count == 1 and .requests[0].user.id == $r
  and .requests[0].channel_id == $c and (.requests[0].messages | length == 2)
  and (.requests[0].messages[0].content | startswith("hi @someone"))
  and .requests[0].messages[1].pictures == 1 and (.requests[0].messages[1] | has("attachments") | not)' >/dev/null
wait_for /tmp/gw-req-pia.jsonl '.t == "MESSAGE_REQUESTS" and .d.count == 1'
grep -v '^$' /tmp/gw-req-pia.jsonl | jq -e --arg c "$RDMID" 'select(.t == "MESSAGE_CREATE" and .d.channel_id == $c)' >/dev/null && { echo "the request reached her gateway"; exit 1; }
# Rex sees his messages normally, with the request noted.
tok "$TOKEN_R" call GET "/api/channels/$RDMID/messages" | jq -e 'length == 2' >/dev/null
tok "$TOKEN_R" call GET /api/dms | jq -e --arg id "$RDMID" '.[] | select(.id == $id) | .request == "outgoing"' >/dev/null
# At most 3 messages to a request that hasn't been accepted.
tok "$TOKEN_R" call POST "/api/channels/$RDMID/messages" '{"content":"third"}' >/dev/null
[ "$(tok "$TOKEN_R" status POST "/api/channels/$RDMID/messages" '{"content":"fourth"}')" = 429 ] && grep -q "Wait for them to accept" /tmp/body
tok "$TOKEN_R" call GET "/api/users/$PIA/contact" | jq -e '(.can_message | not) and .request and (.reason | test("Wait for them"))' >/dev/null
# She can see who's asking (no name history or outhouses: they share none).
tok "$TOKEN_P" call GET "/api/users/$REX/profile/full" | jq -e '.level == "limited" and .names == null and .outhouses == [] and .about_me == ""' >/dev/null
# Accepting: she joins and gets everything (unread), and it's a normal DM for him.
tok "$TOKEN_P" call POST "/api/dms/requests/$RDMID/accept" | jq -e '.request == null and (.recipients | length == 2)' >/dev/null
tok "$TOKEN_P" call GET "/api/channels/$RDMID/messages" | jq -e 'length == 3' >/dev/null
tok "$TOKEN_P" call GET /api/users/@me/unread | jq -e --arg id "$RDMID" 'any(.channels[]; .channel_id == $id)' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
wait_for /tmp/gw-req-pia.jsonl '.t == "MESSAGE_REQUESTS" and .d.count == 0'
tok "$TOKEN_R" call GET /api/dms | jq -e --arg id "$RDMID" '.[] | select(.id == $id) | .request == null' >/dev/null
tok "$TOKEN_R" call POST "/api/channels/$RDMID/messages" '{"content":"thanks!"}' >/dev/null
wait_for /tmp/gw-req-pia.jsonl '.t == "MESSAGE_CREATE" and .d.content == "thanks!"'
tok "$TOKEN_A" call GET "/api/safety/users/$REX" | jq -e '.message_requests.sent == 1 and .message_requests.accepted == 1' >/dev/null

step "message requests: ignore (sender isn't told), block, report; outhouse-mates aren't held"
[ "$(xreg 14 '{"username":"sid","email":"sid@example.test","password":"password123","display_name":"Sid","birth_date":"1990-06-06"}')" = 200 ]
TOKEN_SI=$(jq -r .token /tmp/body); SID=$(jq -r .user.id /tmp/body)
SDM=$(tok "$TOKEN_SI" call POST /api/dms '{"user_ids":["'"$PIA"'"]}' | jq -r .id)
tok "$TOKEN_SI" call POST "/api/channels/$SDM/messages" '{"content":"cheap gold, PM me"}' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 1' >/dev/null
tok "$TOKEN_P" call POST "/api/dms/requests/$SDM/ignore" >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
tok "$TOKEN_SI" call GET /api/dms | jq -e --arg id "$SDM" '.[] | select(.id == $id) | .request == "outgoing"' >/dev/null
tok "$TOKEN_SI" call POST "/api/channels/$SDM/messages" '{"content":"still there?"}' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
tok "$TOKEN_P" fail_call POST "/api/dms/requests/$SDM/accept" # ignored is gone from her list
[ "$(xreg 15 '{"username":"tom","email":"tom@example.test","password":"password123","display_name":"Tom","birth_date":"1990-07-07"}')" = 200 ]
TOKEN_TO=$(jq -r .token /tmp/body); TOM=$(jq -r .user.id /tmp/body)
TOMDM=$(tok "$TOKEN_TO" call POST /api/dms '{"user_ids":["'"$PIA"'"]}' | jq -r .id)
tok "$TOKEN_TO" call POST "/api/channels/$TOMDM/messages" '{"content":"hey"}' >/dev/null
tok "$TOKEN_P" call PUT "/api/users/@me/blocks/$TOM" >/dev/null
[ "$(report "$TOKEN_P" user "$TOM" spam)" = 201 ]
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
[ "$(tok "$TOKEN_TO" status POST "/api/channels/$TOMDM/messages" '{"content":"hello?"}')" = 403 ]
tok "$TOKEN_TO" call GET "/api/users/$PIA/profile/full" | jq -e '.level == "minimal" and (has("about_me") | not) and (.contact.can_message | not)' >/dev/null
tok "$TOKEN_P" call GET "/api/users/$TOM/profile/full" | jq -e '.level == "minimal" and .relationship == "blocked"' >/dev/null
# Unblocked later: his next message is a new request (not lost), which she ignores.
tok "$TOKEN_P" call DELETE "/api/users/@me/blocks/$TOM" >/dev/null
tok "$TOKEN_TO" call POST "/api/channels/$TOMDM/messages" '{"content":"sorry, wrong person"}' >/dev/null
tok "$TOKEN_TO" call GET /api/dms | jq -e --arg id "$TOMDM" '.[] | select(.id == $id) | .request == "outgoing"' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e --arg c "$TOMDM" '.count == 1 and .requests[0].channel_id == $c' >/dev/null
tok "$TOKEN_P" call POST "/api/dms/requests/$TOMDM/ignore" >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$SID" | jq -e '.message_requests.sent == 1 and .message_requests.ignored == 1' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$TOM" | jq -e '.message_requests.sent == 2 and .message_requests.blocked == 1 and .message_requests.ignored == 1' >/dev/null
# Outhouse-mates: delivered as normal.
QDM=$(tok "$TOKEN_Q" call POST /api/dms '{"user_ids":["'"$PIA"'"]}')
jq -e '.request == null and (.recipients | length == 2)' <<<"$QDM" >/dev/null; QDM=$(jq -r .id <<<"$QDM")
tok "$TOKEN_Q" call POST "/api/channels/$QDM/messages" '{"content":"porch tonight?"}' >/dev/null
tok "$TOKEN_P" call GET "/api/channels/$QDM/messages" | jq -e 'length == 1' >/dev/null
# Group DMs: only friends and outhouse-mates, new or existing.
[ "$(xreg 16 '{"username":"uma","email":"uma@example.test","password":"password123","display_name":"Uma","birth_date":"1990-08-08"}')" = 200 ]
TOKEN_U=$(jq -r .token /tmp/body); UMA=$(jq -r .user.id /tmp/body)
tok "$TOKEN_U" call POST "/api/invites/$PINV" >/dev/null
[ "$(tok "$TOKEN_P" status POST /api/dms '{"user_ids":["'"$QUIN"'","'"$REX"'"]}')" = 403 ] && grep -q "Group DMs are only for friends" /tmp/body
PGDM=$(tok "$TOKEN_P" call POST /api/dms '{"user_ids":["'"$QUIN"'","'"$UMA"'"]}' | jq -r .id)
[ "$(tok "$TOKEN_P" status POST "/api/dms/$PGDM/recipients" '{"user_id":"'"$REX"'"}')" = 403 ] && grep -q "Group DMs are only for friends" /tmp/body

step "message requests: friending or sharing an outhouse accepts them; older conversations are grandfathered"
# Sid's ignored request goes through once they're friends.
tok "$TOKEN_SI" call PUT "/api/users/@me/relationships/$PIA" | jq -e '.kind == "outgoing"' >/dev/null
tok "$TOKEN_P" call PUT "/api/users/@me/relationships/$SID" | jq -e '.kind == "friend"' >/dev/null
tok "$TOKEN_P" call GET /api/dms | jq -e --arg id "$SDM" 'any(.[]; .id == $id)' >/dev/null
tok "$TOKEN_P" call GET "/api/channels/$SDM/messages" | jq -e 'length == 2' >/dev/null
tok "$TOKEN_SI" call GET /api/dms | jq -e --arg id "$SDM" '.[] | select(.id == $id) | .request == null' >/dev/null
# A new one, then joining her outhouse.
[ "$(xreg 17 '{"username":"vic","email":"vic@example.test","password":"password123","display_name":"Vic","birth_date":"1990-09-09"}')" = 200 ]
TOKEN_V=$(jq -r .token /tmp/body); VIC=$(jq -r .user.id /tmp/body)
VDM=$(tok "$TOKEN_V" call POST /api/dms '{"user_ids":["'"$PIA"'"]}' | jq -r .id)
tok "$TOKEN_V" call POST "/api/channels/$VDM/messages" '{"content":"hello from vic"}' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 1' >/dev/null
tok "$TOKEN_V" call POST "/api/invites/$PINV" >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null
tok "$TOKEN_P" call GET "/api/channels/$VDM/messages" | jq -e 'length == 1' >/dev/null
# Both have talked: still delivered after they stop sharing an outhouse.
tok "$TOKEN_P" call POST "/api/channels/$QDM/messages" '{"content":"yes!"}' >/dev/null
tok "$TOKEN_Q" call POST "/api/guilds/$PG/leave" >/dev/null
tok "$TOKEN_Q" call POST "/api/channels/$QDM/messages" '{"content":"see you there"}' >/dev/null
tok "$TOKEN_P" call GET "/api/channels/$QDM/messages" | jq -e 'length == 3' >/dev/null
# Only one side ever talked: the next message after they stop sharing is a request again.
UDM=$(tok "$TOKEN_U" call POST /api/dms '{"user_ids":["'"$PIA"'"]}' | jq -r .id)
tok "$TOKEN_U" call POST "/api/channels/$UDM/messages" '{"content":"first"}' >/dev/null
tok "$TOKEN_U" call POST "/api/guilds/$PG/leave" >/dev/null
tok "$TOKEN_U" call GET "/api/users/$PIA/contact" | jq -e '.can_message and .request' >/dev/null
tok "$TOKEN_U" call POST "/api/channels/$UDM/messages" '{"content":"second"}' >/dev/null
tok "$TOKEN_U" call GET /api/dms | jq -e --arg id "$UDM" '.[] | select(.id == $id) | .request == "outgoing" and (.recipients | length == 2)' >/dev/null
tok "$TOKEN_P" call GET /api/dms | jq -e --arg id "$UDM" 'all(.[]; .id != $id)' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e --arg c "$UDM" '.count == 1 and .requests[0].channel_id == $c and (.requests[0].messages | length == 2)' >/dev/null
# Messaging them back from her side accepts it.
tok "$TOKEN_P" call POST /api/dms '{"user_ids":["'"$UMA"'"]}' | jq -e --arg id "$UDM" '.id == $id and .request == null' >/dev/null
tok "$TOKEN_P" call GET /api/dms/requests | jq -e '.count == 0' >/dev/null

step "message requests: the privacy setting, the daily limit, and bots are never held"
tok "$TOKEN_P" call GET /api/users/@me/privacy | jq -e '.message_requests == "allow"' >/dev/null
tok "$TOKEN_P" call PUT /api/users/@me/privacy '{"message_requests":"block"}' | jq -e '.message_requests == "block" and .allow_dms == "everyone"' >/dev/null
tok "$TOKEN_P" fail_call PUT /api/users/@me/privacy '{"message_requests":"maybe"}'
[ "$(xreg 18 '{"username":"wes","email":"wes@example.test","password":"password123","display_name":"Wes","birth_date":"1990-10-10"}')" = 200 ]
TOKEN_W=$(jq -r .token /tmp/body); WES=$(jq -r .user.id /tmp/body)
[ "$(tok "$TOKEN_W" status POST /api/dms '{"user_ids":["'"$PIA"'"]}')" = 403 ] && grep -q "only takes messages from friends and people in their outhouses" /tmp/body
tok "$TOKEN_W" call GET "/api/users/$PIA/contact" | jq -e '(.can_message | not) and .request' >/dev/null
tok "$TOKEN_P" call PUT /api/users/@me/privacy '{"message_requests":"allow"}' >/dev/null
# MESSAGE_REQUESTS_PER_DAY is 4 in this test.
for who in "$QUIN" "$REX" "$SID" "$TOM"; do
  D=$(tok "$TOKEN_W" call POST /api/dms '{"user_ids":["'"$who"'"]}' | jq -r .id)
  tok "$TOKEN_W" call POST "/api/channels/$D/messages" '{"content":"hi, I am Wes"}' >/dev/null
done
[ "$(tok "$TOKEN_W" status POST /api/dms '{"user_ids":["'"$VIC"'"]}')" = 429 ] && grep -q "message requests today" /tmp/body
tok "$TOKEN_W" call GET "/api/users/$VIC/contact" | jq -e '(.can_message | not) and (.reason | test("today"))' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/users/$WES" | jq -e '.message_requests.sent == 4 and .message_requests.last_day == 4' >/dev/null
# Outhouse Safety (the server's helper) reaches Wes straight away.
tok "$TOKEN_A" call POST "/api/safety/users/$WES/actions" '{"action":"warn","text":"Please slow down with the DMs."}' >/dev/null
tok "$TOKEN_W" call GET /api/dms | jq -e 'any(.[]; .request == null and any(.recipients[]; .bot))' >/dev/null

step "profiles: visibility for teens, blocks, suspensions and the site owner"
# Fran (adult, not her friend) sees only Tia's name, picture and outhouses in common.
tok "$TOKEN_F" call GET "/api/users/$TIA/profile/full" | jq -e '.level == "teen" and (has("about_me") | not) and (has("names") | not)
  and (has("friends_in_common") | not) and (has("characters") | not) and (.outhouses | length >= 1) and (.outhouses[0] | has("roles") | not)' >/dev/null
tok "$TOKEN_T" call GET "/api/users/$FRAN/profile/full" | jq -e '.level == "full"' >/dev/null
# Tia blocked Adam (above): just a name and picture either way.
tok "$TOKEN_AD" call GET "/api/users/$TIA/profile/full" | jq -e '.level == "minimal" and (has("outhouses") | not)' >/dev/null
tok "$TOKEN_T" call GET "/api/users/$ADAM/profile/full" | jq -e '.level == "minimal" and .relationship == "blocked"' >/dev/null
tok "$TOKEN_A" call GET "/api/users/$TIA/profile/full" | jq -e '.level == "full" and .site_owner_view' >/dev/null
# Suspended: "This account is suspended" to everyone but the owner.
tok "$TOKEN_A" call POST "/api/safety/users/$VIC/actions" '{"action":"suspend","days":1,"text":"profile test"}' >/dev/null
tok "$TOKEN_P" call GET "/api/users/$VIC/profile/full" | jq -e '.level == "suspended" and .suspended and .avatar_url == null and (has("about_me") | not)' >/dev/null
tok "$TOKEN_P" call GET "/api/users/$VIC/contact" | jq -e '(.can_message | not) and .reason == "This account is suspended."' >/dev/null
tok "$TOKEN_A" call GET "/api/users/$VIC/profile/full" | jq -e '.level == "full" and .suspended' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$VIC/actions" '{"action":"unsuspend"}' >/dev/null
TOKEN=""; fail_call GET "/api/users/$PIA/profile/full"; TOKEN=$TOKEN_A

# ---------------------------------------------------------------------------
# The image detector (feat-detector): a local model in a sidecar, test mode
# (a PNG with a marker tEXt chunk scores 0.97; everything else is low).
# No real explicit pictures anywhere.
# ---------------------------------------------------------------------------
DT=/tmp/dt
dtupload() { # dtupload TOKEN CHANNEL FILE TEXT -> HTTP status (message JSON in /tmp/body)
  curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $1" -F "content=$4" -F "files=@$3;type=image/png" "$BASE/api/channels/$2/messages/upload"
}
dtkey() { psql "$DATABASE_URL" -tAc "SELECT storage_key FROM attachments WHERE message_id = $1 ORDER BY id LIMIT 1"; }
dtscan() { psql "$DATABASE_URL" -tAc "SELECT status || ',' || hold FROM file_scans WHERE storage_key = '$1'"; }
dtwait() { # dtwait KEY "status,hold" -> waits up to 30 s
  for _ in $(seq 1 120); do [ "$(dtscan "$1")" = "$2" ] && return 0; sleep 0.25; done
  echo "scan of $1 is '$(dtscan "$1")', wanted '$2'"; tail -20 "$LOG"; return 1
}
dtreport() { psql "$DATABASE_URL" -tAc "SELECT r.id FROM reports r JOIN file_scans f ON f.id = r.target_id WHERE r.target_kind = 'file' AND f.storage_key = '$1' AND r.status <> 'closed'"; }
dtfile() { curl -sS -o /tmp/dtfile -w "%{http_code}" ${2:+-H "Authorization: Bearer $2"} "$BASE$1"; }

step "detector: owner-only settings, switched on"
tok "$TOKEN_B" fail_call GET /api/safety/detector
tok "$TOKEN_B" fail_call PUT /api/safety/detector '{"enabled":true}'
tok "$TOKEN_A" fail_call PUT /api/safety/detector '{"sensitivity":"extreme"}'
tok "$TOKEN_A" call PUT /api/safety/detector '{"enabled":true}' | jq -e '.enabled and .sensitivity == "medium" and .action == "hold" and .hold_teens' >/dev/null
tok "$TOKEN_A" call GET /api/safety/detector | jq -e '.health.configured and .health.online and .health.test_mode and .thresholds.medium == 0.6 and (.outhouses | length) >= 1' >/dev/null
DG=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Detector Test"}' | jq -r .id)
DCH=$(tok "$TOKEN_A" call GET "/api/guilds/$DG/channels" | jq -r '[.[] | select(.kind == 0)][0].id')
DINV=$(tok "$TOKEN_A" call POST "/api/guilds/$DG/invites" | jq -r .code)
for t in "$TOKEN_B" "$TOKEN_D"; do tok "$t" call POST "/api/invites/$DINV" >/dev/null; done
tok "$TOKEN_B" fail_call GET "/api/safety/guilds/$DG/detector"
tok "$TOKEN_A" call GET "/api/safety/guilds/$DG/detector" | jq -e '.enabled and .sensitivity == null and .effective.sensitivity == "medium" and .hold_teens' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '.detector.configured and .detector.online and .detector.queue >= 0' >/dev/null

step "detector: a clean picture is shown as normal"
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/plain.png" "holiday snap")" = 200 ]
DM0=$(jq -r .id /tmp/body); DK0=$(dtkey "$DM0")
jq -e '.attachments[0].url != "" and (.attachments[0].hold == null)' /tmp/body >/dev/null   # adults only: no waiting
dtwait "$DK0" "clean,none"
psql "$DATABASE_URL" -tAc "SELECT score < 0.1 AND model LIKE 'nudenet-320n%' AND scanned_at IS NOT NULL FROM file_scans WHERE storage_key = '$DK0'" | grep -qx t
[ "$(dtfile "/files/$DK0/plain.png")" = 200 ] && cmp -s /tmp/dtfile "$DT/plain.png"

step "detector: flagged in an ordinary channel -> held for review + a detector report"
rm -f /tmp/gw-dt-dora.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_D" /tmp/gw-dt-dora.jsonl 90 & DTP1=$!
wait_for /tmp/gw-dt-dora.jsonl '.t == "READY"'
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/flag.png" "look")" = 200 ]
DM1=$(jq -r .id /tmp/body); DK1=$(dtkey "$DM1"); DA1=$(jq -r '.attachments[0].id' /tmp/body)
dtwait "$DK1" "flagged,held"
for t in "$TOKEN_B" "$TOKEN_D" "$TOKEN_A"; do
  tok "$t" call GET "/api/channels/$DCH/messages?limit=20" | jq -e --arg m "$DM1" '.[] | select(.id == $m) | .attachments[0].url == "" and .attachments[0].hold.state == "held" and .attachments[0].hold.reveal == "sender_owner"' >/dev/null
done
wait_for /tmp/gw-dt-dora.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $m and .d.attachments[0].hold.state == "held" and .d.attachments[0].url == ""' --arg m "$DM1"
# Nobody gets it by its link; the sender and the site owner can look (and that's logged).
[ "$(dtfile "/files/$DK1/flag.png")" = 451 ] && grep -q "held for review" /tmp/dtfile
[ "$(dtfile "/api/attachments/$DA1/reveal" "$TOKEN_D")" = 451 ]
[ "$(dtfile "/api/attachments/$DA1/reveal" "$TOKEN_B")" = 200 ] && cmp -s /tmp/dtfile "$DT/flag.png"
[ "$(dtfile "/api/attachments/$DA1/reveal" "$TOKEN_A")" = 200 ]
[ "$(dtfile "/api/attachments/$DA1/reveal" "$OUT")" = 404 ]
tok "$TOKEN_A" call GET "/api/safety/log" | jq -e '.entries[0].action == "view_held"' >/dev/null
# Can't be forwarded while it's held.
[ "$(tok "$TOKEN_B" status POST "/api/channels/$DCH/messages/$DM1/forward" '{"destinations":["'"$DCH"'"]}')" = 403 ] && grep -q "held for review" /tmp/body
DR1=$(dtreport "$DK1"); [ -n "$DR1" ]
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg r "$DR1" '.reports[] | select(.id == $r) | .source == "detector" and .category == "sexual"
  and (.child_safety | not) and (.preview | startswith("Detector: 0.97")) and .reported_user.username == "bob"' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$DR1" | jq -e --arg k "$DK1" '.detector.status == "flagged" and .detector.hold == "held" and .detector.score > 0.9
  and (.detector.teen | not) and .detector.viewable and (.files | length) == 1 and .files[0].key == $k and .files[0].viewable
  and (.files[0].detector.summary | startswith("Detector: 0.97")) and .snapshot.detector.score > 0.9 and (.reporters | length) == 0' >/dev/null
tok "$TOKEN_B" fail_call POST "/api/safety/reports/$DR1/actions" '{"action":"approve_file"}'

step "detector: approving releases it live to open clients"
act "$DR1" '{"action":"approve_file"}' | jq -e '.report.status == "closed" and (.report.resolution | test("Approved")) and .detector.review == "approved" and .detector.hold == "none"' >/dev/null
wait_for /tmp/gw-dt-dora.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $m and .d.attachments[0].url != "" and .d.attachments[0].hold == null' --arg m "$DM1"
[ "$(dtfile "/files/$DK1/flag.png")" = 200 ]
[ -f "$UPLOADS/$DK1" ] || { echo "an approved picture was deleted when its report closed"; exit 1; }
kill $DTP1 2>/dev/null || true

step "detector: an 18+ channel where everyone is an adult -> posted as normal"
DNS=$(tok "$TOKEN_A" call POST "/api/guilds/$DG/channels" '{"name":"grown-ups"}' | jq -r .id)
tok "$TOKEN_A" call PATCH "/api/channels/$DNS" '{"content_mode":"nsfw"}' >/dev/null
tok "$TOKEN_B" call PUT "/api/channels/$DNS/age-gate" >/dev/null
[ "$(dtupload "$TOKEN_B" "$DNS" "$DT/flag.png" "fine in here")" = 200 ]
DM2=$(jq -r .id /tmp/body); DK2=$(dtkey "$DM2")
dtwait "$DK2" "flagged,none"
psql "$DATABASE_URL" -tAc "SELECT adult_only AND NOT teen FROM file_scans WHERE storage_key = '$DK2'" | grep -qx t
tok "$TOKEN_B" call GET "/api/channels/$DNS/messages" | jq -e --arg m "$DM2" '.[] | select(.id == $m) | .attachments[0].url != "" and .attachments[0].hold == null' >/dev/null
[ "$(dtfile "/files/$DK2/flag.png")" = 200 ]
[ -z "$(dtreport "$DK2")" ] || { echo "a picture in an adult-only 18+ channel was reported"; exit 1; }
# It can only be forwarded to another adult-only 18+ channel.
[ "$(tok "$TOKEN_B" status POST "/api/channels/$DNS/messages/$DM2/forward" '{"destinations":["'"$DCH"'"]}')" = 403 ]

step "detector: sent to a teen -> fully held (the teen never gets it, nobody can look)"
# Tia blocked Adam at the end of the safety section: make them friends again.
tok "$TOKEN_T" call DELETE "/api/users/@me/blocks/$ADAM" >/dev/null
tok "$TOKEN_A" call PUT "/api/safety/guilds/$SG/family" '{"family":true}' >/dev/null
tok "$TOKEN_AD" status PUT "/api/users/@me/relationships/$TIA" >/dev/null
tok "$TOKEN_T" status PUT "/api/users/@me/relationships/$ADAM" >/dev/null
tok "$TOKEN_A" call PUT "/api/safety/guilds/$SG/family" '{"family":false}' >/dev/null
rm -f /tmp/gw-dt-tia.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_T" /tmp/gw-dt-tia.jsonl 90 & DTP2=$!
wait_for /tmp/gw-dt-tia.jsonl '.t == "READY"'
[ "$(dtupload "$TOKEN_AD" "$TDM" "$DT/flag.png" "for you")" = 200 ]
DM3=$(jq -r .id /tmp/body); DK3=$(dtkey "$DM3"); DA3=$(jq -r '.attachments[0].id' /tmp/body)
# Held as "Checking…" from the start (pictures to or from teens wait for the check).
jq -e '.attachments[0].url == "" and .attachments[0].hold.state == "checking"' /tmp/body >/dev/null
dtwait "$DK3" "flagged,held"
psql "$DATABASE_URL" -tAc "SELECT teen AND NOT teen_author FROM file_scans WHERE storage_key = '$DK3'" | grep -qx t
tok "$TOKEN_T" call GET "/api/channels/$TDM/messages" | jq -e --arg m "$DM3" '.[] | select(.id == $m) | .attachments[0].url == "" and .attachments[0].hold.state == "held" and .attachments[0].hold.reveal == "none"' >/dev/null
for t in "$TOKEN_T" "$TOKEN_AD" "$TOKEN_A"; do [ "$(dtfile "/api/attachments/$DA3/reveal" "$t")" = 451 ]; done
[ "$(dtfile "/files/$DK3/flag.png")" = 451 ]
sleep 1
jq -r 'select(.t == "MESSAGE_CREATE" or .t == "MESSAGE_UPDATE") | .d.attachments[]?.url' /tmp/gw-dt-tia.jsonl | grep -q "$DK3" && { echo "the teen was sent the held picture's link"; exit 1; }
DR3=$(dtreport "$DK3"); [ -n "$DR3" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$DR3" | jq -e '.detector.teen and (.detector.viewable | not) and (.files[0].viewable | not) and .files[0].teen_hold and (.report.child_safety | not)' >/dev/null
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$DR3/files/$DK3")" = 451 ]
# Keeping it: it stays held (never just blurred) because a teen is involved.
act "$DR3" '{"action":"keep_blurred"}' | jq -e '.detector.hold == "held" and .report.status == "closed"' >/dev/null
# From a teen: held for the adult too.
[ "$(dtupload "$TOKEN_T" "$TDM" "$DT/flag.png" "from me")" = 200 ]
DK4=$(dtkey "$(jq -r .id /tmp/body)"); dtwait "$DK4" "flagged,held"
psql "$DATABASE_URL" -tAc "SELECT teen_author FROM file_scans WHERE storage_key = '$DK4'" | grep -qx t
# A clean picture to a teen shows once it's been checked (slow on purpose: "Checking…" first).
[ "$(dtupload "$TOKEN_AD" "$TDM" "$DT/slow.png" "dinner")" = 200 ]
DM5=$(jq -r .id /tmp/body); DK5=$(dtkey "$DM5")
jq -e '.attachments[0].hold.state == "checking"' /tmp/body >/dev/null
[ "$(dtfile "/files/$DK5/slow.png")" = 451 ]
dtwait "$DK5" "clean,none"
wait_for /tmp/gw-dt-tia.jsonl '.t == "MESSAGE_UPDATE" and .d.id == $m and .d.attachments[0].url != "" and .d.attachments[0].hold == null' --arg m "$DM5"
kill $DTP2 2>/dev/null || true
# A channel a teen can see counts too (Tia is in the first test outhouse).
[ "$(dtupload "$TOKEN_B" "$CHAN" "$DT/flag.png" "in general")" = 200 ]
jq -e '.attachments[0].hold.state == "checking"' /tmp/body >/dev/null
DK6=$(dtkey "$(jq -r .id /tmp/body)"); dtwait "$DK6" "flagged,held"
psql "$DATABASE_URL" -tAc "SELECT teen FROM file_scans WHERE storage_key = '$DK6'" | grep -qx t

step "detector: blur only (click through), per-outhouse settings, switched off"
tok "$TOKEN_A" call PUT "/api/safety/guilds/$DG/detector" '{"action":"blur"}' | jq -e '.action == "blur" and .effective.action == "blur"' >/dev/null
[ "$(dtupload "$TOKEN_D" "$DCH" "$DT/flag.png" "blurry")" = 200 ]
DM7=$(jq -r .id /tmp/body); DK7=$(dtkey "$DM7")
dtwait "$DK7" "flagged,blur"
tok "$TOKEN_B" call GET "/api/channels/$DCH/messages" | jq -e --arg m "$DM7" '.[] | select(.id == $m) | .attachments[0].url != "" and .attachments[0].hold.state == "blur" and .attachments[0].hold.reveal == "anyone"' >/dev/null
[ "$(dtfile "/files/$DK7/flag.png")" = 200 ]
[ -n "$(dtreport "$DK7")" ]
tok "$TOKEN_A" call PUT "/api/safety/guilds/$DG/detector" '{"action":"default","enabled":false}' | jq -e '(.enabled | not) and .action == null' >/dev/null
[ "$(dtupload "$TOKEN_D" "$DCH" "$DT/flag.png" "not checked")" = 200 ]
DK8=$(dtkey "$(jq -r .id /tmp/body)"); dtwait "$DK8" "skipped,none"
tok "$TOKEN_A" call PUT "/api/safety/guilds/$DG/detector" '{"enabled":true}' >/dev/null
tok "$TOKEN_A" call PUT "/api/safety/detector" '{"sensitivity":"low"}' | jq -e '.sensitivity == "low"' >/dev/null
tok "$TOKEN_A" call PUT "/api/safety/detector" '{"sensitivity":"medium"}' >/dev/null

step "detector: avatars, emoji and stickers are hidden (placeholder) until approved"
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_D" -F "file=@$DT/avatar-flag.png;type=image/png" "$BASE/api/users/@me/avatar")
[ "$code" = 200 ]; AVK=$(jq -r '.avatar_url // empty' /tmp/body | cut -d/ -f3)
[ -n "$AVK" ] || AVK=$(psql "$DATABASE_URL" -tAc "SELECT avatar_key FROM users WHERE id = $DORA_ID")
dtwait "$AVK" "flagged,hidden"
code=$(curl -sS -D /tmp/headers -o /tmp/dtfile -w "%{http_code}" "$BASE/files/$AVK/avatar")
[ "$code" = 200 ] && grep -qi "content-type: image/svg+xml" /tmp/headers && grep -qi "cache-control: no-store" /tmp/headers && grep -q "<svg" /tmp/dtfile
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "name=dtflag" -F "file=@$DT/avatar-flag.png;type=image/png" "$BASE/api/guilds/$DG/emojis")
[ "$code" = 200 ]; DEMO=$(jq -r .id /tmp/body); DEK=$(psql "$DATABASE_URL" -tAc "SELECT storage_key FROM emojis WHERE id = $DEMO")
dtwait "$DEK" "flagged,hidden"
curl -fsS "$BASE/emojis/$DEMO" | grep -q "<svg"
DRA=$(dtreport "$AVK"); [ -n "$DRA" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$DRA" | jq -e '.detector.kind == "avatar" and .report.target_kind == "file" and .snapshot.kind == "user"' >/dev/null
act "$DRA" '{"action":"approve_file"}' >/dev/null
curl -fsS "$BASE/files/$AVK/avatar" -o /tmp/dtfile && cmp -s /tmp/dtfile "$DT/avatar-flag.png"
act "$(dtreport "$DEK")" '{"action":"remove_file"}' | jq -e '.report.status == "closed" and (.report.resolution | test("Removed"))' >/dev/null
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/emojis/$DEMO")" = 404 ]

step "outhouse picture and banner go through the image detector like avatars"
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_B" -F "file=@$DT/plain.png;type=image/png" "$BASE/api/guilds/$DG/icon")
[ "$code" = 403 ]   # Bob can't manage the outhouse
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "file=@$DT/avatar-flag.png;type=image/png" "$BASE/api/guilds/$DG/icon")
[ "$code" = 200 ]; GIK=$(jq -r '.icon_url' /tmp/body | cut -d/ -f3); [ -n "$GIK" ] && [ "$GIK" != null ]
dtwait "$GIK" "flagged,hidden"
psql "$DATABASE_URL" -tAc "SELECT kind = 'guild_icon' AND guild_id = $DG FROM file_scans WHERE storage_key = '$GIK'" | grep -qx t
code=$(curl -sS -D /tmp/headers -o /tmp/dtfile -w "%{http_code}" "$BASE/files/$GIK/icon")
[ "$code" = 200 ]; grep -qi "content-type: image/svg+xml" /tmp/headers && grep -q "<svg" /tmp/dtfile
DRG=$(dtreport "$GIK"); [ -n "$DRG" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$DRG" | jq -e --arg g "$DG" '.detector.kind == "guild_icon" and .snapshot.kind == "guild" and .snapshot.guild.id == $g' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT reported_user_id = $ALICE AND guild_id = $DG FROM reports WHERE id = $DRG")" = t ]
act "$DRG" '{"action":"remove_file"}' | jq -e '.report.status == "closed"' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT icon_key IS NULL FROM guilds WHERE id = $DG")" = t ]
tok "$TOKEN_B" call GET "/api/guilds/$DG/info" | jq -e '.icon_url == null' >/dev/null
# A clean banner shows as normal; removing it clears it.
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "file=@$DT/plain.png;type=image/png" "$BASE/api/guilds/$DG/banner")
[ "$code" = 200 ]; GBK=$(jq -r '.banner_url' /tmp/body | cut -d/ -f3)
dtwait "$GBK" "clean,none"
[ "$(dtfile "/files/$GBK/banner")" = 200 ]; cmp -s /tmp/dtfile "$DT/plain.png"
tok "$TOKEN_A" call DELETE "/api/guilds/$DG/banner" | jq -e '.banner_url == null' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$DG/audit-log?action=guild_update" | jq -e '[.entries[] | select(.details | test("banner"))] | length >= 2' >/dev/null

step "detector: a child-safety report shows the detector score while the file stays quarantined (451, owner included)"
[ "$(dtupload "$TOKEN_D" "$DCH" "$DT/plain2.png" "a cat")" = 200 ]
DM9=$(jq -r .id /tmp/body); DK9=$(dtkey "$DM9"); dtwait "$DK9" "clean,none"
# Never scanned before (as if uploaded while the detector was off): it's scanned for the review screen.
psql "$DATABASE_URL" -qc "DELETE FROM file_scans WHERE storage_key = '$DK9'"
[ "$(report "$TOKEN_B" message "$DM9" child_safety "false alarm test")" = 201 ]; DR9=$(report_id message "$DM9")
for _ in $(seq 1 60); do
  tok "$TOKEN_A" call GET "/api/safety/reports/$DR9" > /tmp/dtr.json
  jq -e '.files[0].detector.status == "clean"' /tmp/dtr.json >/dev/null && break; sleep 0.5
done
jq -e '.report.child_safety and .files[0].quarantined and (.files[0].viewable | not) and .files[0].detector.near_zero
  and (.files[0].detector.summary | test("^Detector: 0\\.0[0-9] — nothing explicit found$"))' /tmp/dtr.json >/dev/null
psql "$DATABASE_URL" -tAc "SELECT kind || ',' || apply_hold || ',' || hold FROM file_scans WHERE storage_key = '$DK9'" | grep -qx "report,false,none"
for who in "" "$TOKEN_A" "$TOKEN_D"; do [ "$(dtfile "/files/$DK9/plain2.png" "$who")" = 451 ]; done
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$DR9/files/$DK9")" = 451 ]
[ "$(dtfile "/api/attachments/$(psql "$DATABASE_URL" -tAc "SELECT id FROM attachments WHERE storage_key = '$DK9'")/reveal" "$TOKEN_A")" = 451 ]
act "$DR9" '{"action":"dismiss","text":"Detector 0.0x, a cat"}' >/dev/null

step "detector: when it's down uploads still work, Server status says offline, and the queue catches up"
detector_stop
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '.detector.configured and (.detector.online | not)' >/dev/null
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/plain.png" "while it's down")" = 200 ]
jq -e '.attachments[0].url != ""' /tmp/body >/dev/null              # adults: shown while waiting
DK10=$(dtkey "$(jq -r .id /tmp/body)")
[ "$(dtupload "$TOKEN_AD" "$TDM" "$DT/plain.png" "to a teen while it's down")" = 200 ]
jq -e '.attachments[0].hold.state == "checking"' /tmp/body >/dev/null   # teens: held until checked
DK11=$(dtkey "$(jq -r .id /tmp/body)")
for _ in $(seq 1 40); do
  [ "$(psql "$DATABASE_URL" -tAc "SELECT attempts > 0 AND error LIKE 'The detector didn''t answer%' FROM file_scans WHERE storage_key = '$DK10'")" = t ] && break; sleep 0.5
done
dtscan "$DK10" | grep -qx "pending,none"
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '(.detector.online | not) and .detector.queue >= 2 and (.detector.last_error | length) > 0' >/dev/null
detector_start
tok "$TOKEN_A" call PUT /api/safety/detector '{}' >/dev/null      # saving the settings retries straight away
dtwait "$DK10" "clean,none"; dtwait "$DK11" "clean,none"
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '.detector.online and (.detector.last_error == null)' >/dev/null
# Leave it as the UI test expects: on, medium, hold for approval.
tok "$TOKEN_A" call PUT /api/safety/detector '{"enabled":true,"sensitivity":"medium","action":"hold","hold_teens":true}' >/dev/null

step "detector: several workers (DETECTOR_WORKERS=2) -> a burst of 10 uploads is scanned exactly once each"
if [ "${DETECTOR_WORKERS:-1}" -ge 2 ]; then
  tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e --argjson n "$DETECTOR_WORKERS" '.detector.workers.server == $n and .detector.workers.total == $n
    and .detector.workers.busy >= 0 and (.detector.workers.memory_mb.per_worker > 0) and (.detector.backlog_hint | not)' >/dev/null
fi
BPIDS=""
for i in $(seq 0 9); do
  curl -sS -o "/tmp/burst-$i.json" -H "Authorization: Bearer $TOKEN_B" -F "content=burst $i" -F "files=@$DT/burst-$i.png;type=image/png" \
    "$BASE/api/channels/$DCH/messages/upload" & BPIDS="$BPIDS $!"
done
# shellcheck disable=SC2086
wait $BPIDS
BKEYS=""
for i in $(seq 0 9); do BKEYS="$BKEYS'$(dtkey "$(jq -r .id "/tmp/burst-$i.json")")',"; done
BKEYS="${BKEYS%,}"
for _ in $(seq 1 120); do
  [ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM file_scans WHERE storage_key IN ($BKEYS) AND status IN ('clean', 'flagged') AND claimed_at IS NULL")" = 10 ] && break; sleep 0.25
done
# Each scanned once (attempts = 1), nothing left claimed, 8 clean + 2 flagged and held.
psql "$DATABASE_URL" -tAc "SELECT count(*) FILTER (WHERE status = 'clean' AND hold = 'none') || ',' || count(*) FILTER (WHERE status = 'flagged' AND hold = 'held')
  || ',' || count(*) FILTER (WHERE attempts = 1 AND claimed_at IS NULL) FROM file_scans WHERE storage_key IN ($BKEYS)" | grep -qx "8,2,10" \
  || { psql "$DATABASE_URL" -c "SELECT storage_key, status, hold, attempts, claimed_at FROM file_scans WHERE storage_key IN ($BKEYS)"; tail -20 "$LOG"; exit 1; }
# Exactly one detector report per flagged picture (no duplicates; a claim is
# only released once its scan, hold and report are all done).
psql "$DATABASE_URL" -tAc "SELECT count(*) || ',' || count(DISTINCT r.target_id) FROM reports r JOIN file_scans f ON f.id = r.target_id
  WHERE r.target_kind = 'file' AND f.storage_key IN ($BKEYS)" | grep -qx "2,2"
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '.detector.avg_scan_ms >= 0 and .detector.avg_scan_count >= 10 and .detector.workers.busy == 0' >/dev/null
for r in $(psql "$DATABASE_URL" -tAc "SELECT r.id FROM reports r JOIN file_scans f ON f.id = r.target_id WHERE r.target_kind = 'file' AND r.status <> 'closed' AND f.storage_key IN ($BKEYS)"); do
  act "$r" '{"action":"remove_file"}' >/dev/null
done

# ---------------------------------------------------------------------------
# The age check (feat-age): faces and age estimates from the same sidecar.
# Test-mode markers simulate them: young-flag (0.97 + a face ~8), young (a
# face ~8, nothing explicit), adult-flag (0.97 + a face ~34). Plain coloured
# squares: no people, nothing explicit.
# ---------------------------------------------------------------------------
agereport() { # agereport KEY -> the open child-safety report about this file (any source)
  psql "$DATABASE_URL" -tAc "SELECT r.id FROM reports r WHERE r.status <> 'closed' AND r.category = 'child_safety'
    AND ((r.target_kind = 'file' AND r.target_id = (SELECT id FROM file_scans WHERE storage_key = '$1'))
         OR r.id IN (SELECT report_id FROM report_files WHERE storage_key = '$1')) ORDER BY r.id LIMIT 1"
}
agewait() { # agewait KEY -> waits for the automatic child-safety report
  for _ in $(seq 1 120); do [ -n "$(agereport "$1")" ] && return 0; sleep 0.25; done
  echo "no automatic child-safety report for $1 ($(dtscan "$1"))"; tail -20 "$LOG"; return 1
}

step "age check: owner-only settings, defaults, models in Server status"
tok "$TOKEN_B" fail_call PUT /api/safety/detector '{"age_check":false}'
tok "$TOKEN_A" fail_call PUT /api/safety/detector '{"age_threshold":12}'
tok "$TOKEN_A" fail_call PUT /api/safety/detector '{"escalation_score":1.5}'
tok "$TOKEN_A" call GET /api/safety/detector | jq -e '.age.enabled and .age.threshold == 13 and .age.escalation_score == 0.4
  and .age.thresholds == [10,13,16] and (.health.models.nudity | startswith("nudenet")) and (.health.models.faces | startswith("yunet"))
  and (.health.models.age | startswith("fairface")) and .health.escalation_age_override == null' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$DG/server-status" | jq -e '.detector.models.age != null and .detector.models.faces != null' >/dev/null

step "age check: explicit + a face estimated ~8 -> automatic child-safety report, quarantined (451, owner included), hidden, alert"
NTFY_AGE=$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)
rm -f /tmp/gw-age-dora.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_D" /tmp/gw-age-dora.jsonl 90 & AGP=$!
wait_for /tmp/gw-age-dora.jsonl '.t == "READY"'
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/young-flag.png" "look")" = 200 ]
AM1=$(jq -r .id /tmp/body); AK1=$(dtkey "$AM1"); AA1=$(jq -r '.attachments[0].id' /tmp/body)
agewait "$AK1"; AR1=$(agereport "$AK1")
psql "$DATABASE_URL" -tAc "SELECT face_count = 1 AND min_est_age = 8 AND jsonb_array_length(ages) = 1 AND ages->0 ? 'confidence'
  AND NOT ages->0 ? 'range' AND age_escalated_at IS NOT NULL AND age_model LIKE 'yunet%' FROM file_scans WHERE storage_key = '$AK1'" | grep -qx t
psql "$DATABASE_URL" -tAc "SELECT EXISTS(SELECT 1 FROM quarantined_files WHERE storage_key = '$AK1')" | grep -qx t
for who in "" "$TOKEN_A" "$TOKEN_B" "$TOKEN_D"; do [ "$(dtfile "/files/$AK1/young-flag.png" "$who")" = 451 ]; done
for who in "$TOKEN_A" "$TOKEN_B"; do [ "$(dtfile "/api/attachments/$AA1/reveal" "$who")" = 451 ]; done
[ "$(tok "$TOKEN_A" status GET "/api/safety/reports/$AR1/files/$AK1")" = 451 ]
wait_for /tmp/gw-age-dora.jsonl '.t == "MESSAGE_DELETE" and .d.id == $m' --arg m "$AM1"
kill $AGP 2>/dev/null || true
tok "$TOKEN_D" call GET "/api/channels/$DCH/messages?limit=50" | jq -e --arg m "$AM1" 'all(.[]; .id != $m)' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports?status=open" | jq -e --arg r "$AR1" '.reports[] | select(.id == $r) | .child_safety and .source == "detector" and .priority == 0
  and .preview == "Automatic: explicit content with a face estimated 13 or under" and .auto_escalation' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/reports/$AR1" > /tmp/age1.json
jq -e --arg k "$AK1" '.report.child_safety and .message_hidden and .files[0].key == $k and .files[0].quarantined and (.files[0].viewable | not)
  and .files[0].detector.faces == 1 and .files[0].detector.min_age == 8 and .files[0].detector.age_escalated
  and (.files[0].detector.age_line == "Detector: nudity 0.97 · face 1 · youngest estimated ~8 (preliminary automated estimate, not evidence)")
  and .snapshot.auto_escalation.min_age == 8 and .snapshot.auto_escalation.age_threshold == 13 and (.snapshot.auto_escalation.note | test("not proof"))
  and any(.log[]; .action == "auto_child_safety" and (.details | test("preliminary automated estimate"))) and any(.log[]; .action == "quarantine")' /tmp/age1.json >/dev/null
for _ in $(seq 1 20); do [ "$(curl -fsS "$PUSH/alerts/safety-ntfy" | jq length)" -gt "$NTFY_AGE" ] && break; sleep 0.5; done
curl -fsS "$PUSH/alerts/safety-ntfy" | jq -e '.[-1].title == "Outhouse: urgent safety report" and (.[-1].body | test("Review and suspend the account now if in doubt"))' >/dev/null

step "age check: the evidence package carries the estimates, labelled as preliminary automated estimates"
tok "$TOKEN_A" call GET "/api/safety/reports/$AR1/export" | jq -e '(.age_estimates_note | test("preliminary automated age estimates") and test("NOT evidence"))
  and .files[0].detector.min_age == 8 and .files[0].detector.ages[0].age == 8 and .files[0].detector.age_note == "preliminary automated estimate, not evidence"
  and .snapshot.auto_escalation.line != null' >/dev/null

step "age check: dismissing it as a mistake restores the file (back to the detector's normal hold and report)"
act "$AR1" '{"action":"dismiss","text":"Scores look wrong"}' | jq -e '.report.status == "closed" and (.files[0].quarantined | not) and (.message_hidden | not)
  and .files[0].detector.age_cleared and any(.log[]; .action == "auto_child_safety_dismissed")' >/dev/null
psql "$DATABASE_URL" -tAc "SELECT age_cleared_at IS NOT NULL AND hold = 'held' FROM file_scans WHERE storage_key = '$AK1'" | grep -qx t
tok "$TOKEN_D" call GET "/api/channels/$DCH/messages?limit=50" | jq -e --arg m "$AM1" '.[] | select(.id == $m) | .attachments[0].hold.state == "held"' >/dev/null
[ "$(dtfile "/api/attachments/$AA1/reveal" "$TOKEN_A")" = 200 ] && cmp -s /tmp/dtfile "$DT/young-flag.png"
AR1B=$(dtreport "$AK1"); [ -n "$AR1B" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$AR1B" | jq -e '.report.category == "sexual" and (.report.child_safety | not) and (.report.auto_escalation | not)' >/dev/null
# Opening it again doesn't escalate it again (the owner's judgement stands).
tok "$TOKEN_A" call GET "/api/safety/reports/$AR1B" | jq -e '.report.category == "sexual"' >/dev/null
act "$AR1B" '{"action":"remove_file"}' >/dev/null

step "age check: an adult-only 18+ channel escalates too"
[ "$(dtupload "$TOKEN_B" "$DNS" "$DT/young-flag2.png" "in 18+")" = 200 ]
AM2=$(jq -r .id /tmp/body); AK2=$(dtkey "$AM2")
agewait "$AK2"; AR2=$(agereport "$AK2")
psql "$DATABASE_URL" -tAc "SELECT adult_only AND hold = 'none' FROM file_scans WHERE storage_key = '$AK2'" | grep -qx t
for who in "" "$TOKEN_A" "$TOKEN_B"; do [ "$(dtfile "/files/$AK2/young-flag2.png" "$who")" = 451 ]; done
tok "$TOKEN_B" call GET "/api/channels/$DNS/messages" | jq -e --arg m "$AM2" 'all(.[]; .id != $m)' >/dev/null
act "$AR2" '{"action":"dismiss"}' >/dev/null
[ "$(dtfile "/files/$AK2/young-flag2.png")" = 200 ]
tok "$TOKEN_B" call GET "/api/channels/$DNS/messages" | jq -e --arg m "$AM2" 'any(.[]; .id == $m)' >/dev/null

step "age check: a young face without nudity does nothing; explicit with an adult face is the normal detector path"
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/young.png" "family photo")" = 200 ]
AK3=$(dtkey "$(jq -r .id /tmp/body)"); dtwait "$AK3" "clean,none"
psql "$DATABASE_URL" -tAc "SELECT face_count = 1 AND min_est_age = 8 AND age_escalated_at IS NULL FROM file_scans WHERE storage_key = '$AK3'" | grep -qx t
[ "$(dtfile "/files/$AK3/young.png")" = 200 ]
[ -z "$(dtreport "$AK3")" ] && [ -z "$(agereport "$AK3")" ]
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/adult-flag.png" "adult")" = 200 ]
AK4=$(dtkey "$(jq -r .id /tmp/body)"); dtwait "$AK4" "flagged,held"
psql "$DATABASE_URL" -tAc "SELECT min_est_age = 34 AND age_escalated_at IS NULL FROM file_scans WHERE storage_key = '$AK4'" | grep -qx t
AR4=$(dtreport "$AK4"); [ -n "$AR4" ] && [ -z "$(agereport "$AK4")" ]
tok "$TOKEN_A" call GET "/api/safety/reports/$AR4" | jq -e '.report.category == "sexual" and (.report.child_safety | not) and .detector.min_age == 34
  and (.detector.age_line | test("youngest estimated ~34"))' >/dev/null
act "$AR4" '{"action":"remove_file"}' >/dev/null

step "age check: switched off -> nothing escalated; switched on -> a harassment report on that file is re-classified"
tok "$TOKEN_A" call PUT /api/safety/detector '{"age_check":false}' | jq -e '(.age.enabled | not)' >/dev/null
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/young-flag3.png" "sneaky")" = 200 ]
AM5=$(jq -r .id /tmp/body); AK5=$(dtkey "$AM5")
dtwait "$AK5" "flagged,held"; sleep 1
[ -z "$(agereport "$AK5")" ] || { echo "escalated with the age check off"; exit 1; }
psql "$DATABASE_URL" -tAc "SELECT min_est_age = 8 AND age_escalated_at IS NULL FROM file_scans WHERE storage_key = '$AK5'" | grep -qx t
tok "$TOKEN_A" call PUT /api/safety/detector '{"age_check":true,"age_threshold":13,"escalation_score":0.4}' | jq -e '.age.enabled' >/dev/null
tok "$TOKEN_A" call GET /api/safety/log | jq -e 'any(.entries[]; .action == "settings" and (.details | test("Age check on")))' >/dev/null
[ "$(report "$TOKEN_D" message "$AM5" harassment "rude")" = 201 ]; AR5=$(report_id message "$AM5")
tok "$TOKEN_A" call GET "/api/safety/reports/$AR5" | jq -e '.report.child_safety and .report.auto_escalation and .reporters[0].category == "harassment"
  and .files[0].quarantined and (.files[0].viewable | not) and (.report.preview | startswith("Automatic:"))
  and any(.log[]; .action == "auto_child_safety" and (.details | test("re-classified from “Harassment or bullying” to child safety")))' >/dev/null
[ "$(dtfile "/files/$AK5/young-flag3.png" "$TOKEN_A")" = 451 ]
act "$AR5" '{"action":"dismiss"}' >/dev/null
for r in $(psql "$DATABASE_URL" -tAc "SELECT id FROM reports WHERE status <> 'closed' AND category = 'child_safety' AND id IN (SELECT report_id FROM report_files WHERE storage_key = '$AK5')"); do act "$r" '{"action":"dismiss"}' >/dev/null; done

step "age check: a file scanned before faces were checked is scanned again when it's reported"
[ "$(dtupload "$TOKEN_B" "$DCH" "$DT/young.png" "older upload")" = 200 ]
AM6=$(jq -r .id /tmp/body); AK6=$(dtkey "$AM6"); dtwait "$AK6" "clean,none"
psql "$DATABASE_URL" -qc "UPDATE file_scans SET face_count = NULL, min_est_age = NULL, ages = '[]' WHERE storage_key = '$AK6'"
[ "$(report "$TOKEN_D" message "$AM6" spam "old")" = 201 ]; AR6=$(report_id message "$AM6")
for _ in $(seq 1 60); do [ "$(psql "$DATABASE_URL" -tAc "SELECT face_count FROM file_scans WHERE storage_key = '$AK6'")" = 1 ] && break; sleep 0.25; done
tok "$TOKEN_A" call GET "/api/safety/reports/$AR6" | jq -e '(.report.child_safety | not) and .files[0].detector.min_age == 8 and (.files[0].detector.age_line | test("~8"))' >/dev/null
act "$AR6" '{"action":"dismiss"}' >/dev/null

# ---------------------------------------------------------------------------
# Outhouse folders on the rail (feat-folders): per person, private, synced.
# ---------------------------------------------------------------------------
step "folders: default layout is every outhouse in joined order"
TOKEN=""; fail_call GET /api/users/@me/guild-layout; TOKEN=$TOKEN_A
FF1=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Folder One"}' | jq -r .id)
FF2=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Folder Two"}' | jq -r .id)
FF3=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Folder Three"}' | jq -r .id)
FCG=$(tok "$TOKEN_C" call POST /api/guilds '{"name":"Carl Only"}' | jq -r .id)
NA=$(tok "$TOKEN_A" call GET /api/guilds | jq length)
tok "$TOKEN_A" call GET /api/users/@me/guild-layout > /tmp/lay.json
jq -e --argjson n "$NA" '(.items | length) == $n and all(.items[]; .type == "guild")' /tmp/lay.json >/dev/null
diff <(jq -r '.items[].id' /tmp/lay.json) <(tok "$TOKEN_A" call GET /api/guilds | jq -r '.[].id')

step "folders: create with outhouses in it; strangers' and repeated outhouses are dropped; new ones go at the end"
rm -f /tmp/gw-lay.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_A" /tmp/gw-lay.jsonl 20 & GWL=$!
wait_for /tmp/gw-lay.jsonl '.t == "READY"'
tok "$TOKEN_A" call PUT /api/users/@me/guild-layout '{"items":[{"type":"guild","id":"'"$FCG"'"},{"type":"folder","name":"  Raids  ","color":"blue","guild_ids":["'"$FF2"'","'"$FCG"'","'"$FF1"'","'"$FF2"'"]},{"type":"guild","id":"'"$FF3"'"},{"type":"guild","id":"'"$FF1"'"}]}' > /tmp/lay.json
jq -e --arg a "$FF1" --arg b "$FF2" --arg c "$FF3" --arg x "$FCG" --argjson n "$NA" '
  .items[0].type == "folder" and .items[0].name == "Raids" and .items[0].color == "blue" and (.items[0].collapsed | not)
  and .items[0].guild_ids == [$b, $a] and .items[1] == {"type":"guild","id":$c}
  and (.items | length) == ($n - 1) and ([.. | strings | select(. == $x)] | length) == 0' /tmp/lay.json >/dev/null
FID=$(jq -r '.items[0].id' /tmp/lay.json)
wait_for /tmp/gw-lay.jsonl '.t == "GUILD_LAYOUT_CHANGED"'
kill $GWL 2>/dev/null || true
# Joined later: at the end, outside folders.
FF4=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Folder Four"}' | jq -r .id)
tok "$TOKEN_A" call GET /api/users/@me/guild-layout | jq -e --arg d "$FF4" '.items[-1] == {"type":"guild","id":$d}' >/dev/null

step "folders: rename, colour, collapse and reorder keep the folder's ID"
tok "$TOKEN_A" call PUT /api/users/@me/guild-layout '{"items":[{"type":"guild","id":"'"$FF3"'"},{"type":"folder","id":"'"$FID"'","name":"Raid nights","color":"teal","collapsed":true,"guild_ids":["'"$FF1"'","'"$FF2"'"]}]}' >/dev/null
tok "$TOKEN_A" call GET /api/users/@me/guild-layout | jq -e --arg f "$FID" --arg a "$FF1" --arg b "$FF2" --arg c "$FF3" '
  .items[0] == {"type":"guild","id":$c} and .items[1].id == $f and .items[1].name == "Raid nights" and .items[1].color == "teal"
  and .items[1].collapsed and .items[1].guild_ids == [$a, $b]' >/dev/null
# A made-up folder ID (or someone else's) becomes a new one.
NEWF=$(tok "$TOKEN_A" call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","id":"12345","name":"Mine","guild_ids":[]},{"type":"folder","id":"'"$FID"'","name":"Raid nights","guild_ids":["'"$FF1"'"]}]}' | jq -r '.items[0].id')
[ "$NEWF" != 12345 ] && [ "$NEWF" != "$FID" ]

step "folders: validation (names, colours, limits)"
fail_call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"   ","guild_ids":[]}]}'
fail_call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"'"$(printf 'x%.0s' $(seq 1 33))"'","guild_ids":[]}]}'
fail_call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"A","color":"#ff0000","guild_ids":[]}]}'
fail_call PUT /api/users/@me/guild-layout '{"items":[{"type":"shelf","name":"A"}]}'
fail_call PUT /api/users/@me/guild-layout "$(jq -nc '{items: [range(51) | {type:"folder", name:"F\(.)", guild_ids:[]}]}')"
tok "$TOKEN_A" call PUT /api/users/@me/guild-layout "$(jq -nc '{items: [range(50) | {type:"folder", name:"F\(.)", guild_ids:[]}]}')" | jq -e '[.items[] | select(.type == "folder")] | length == 50' >/dev/null
# Still there after the bad attempts? (a failed save changes nothing)
fail_call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"","guild_ids":[]}]}'
tok "$TOKEN_A" call GET /api/users/@me/guild-layout | jq -e '[.items[] | select(.type == "folder")] | length == 50' >/dev/null

step "folders: private to you; leaving an outhouse takes it out of your folders"
FINV=$(tok "$TOKEN_A" call POST "/api/guilds/$FF1/invites" | jq -r .code)
tok "$TOKEN_B" call POST "/api/invites/$FINV" >/dev/null
tok "$TOKEN_B" call GET /api/users/@me/guild-layout | jq -e '[.items[] | select(.type == "folder")] | length == 0' >/dev/null
tok "$TOKEN_B" call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"Bob stuff","guild_ids":["'"$FF1"'","'"$FF2"'"]}]}' | jq -e --arg a "$FF1" '.items[0].guild_ids == [$a]' >/dev/null
tok "$TOKEN_A" call GET /api/users/@me/guild-layout | jq -e 'all(.items[]; .name != "Bob stuff")' >/dev/null
tok "$TOKEN_B" call POST "/api/guilds/$FF1/leave" >/dev/null
tok "$TOKEN_B" call GET /api/users/@me/guild-layout | jq -e --arg a "$FF1" '.items[0].name == "Bob stuff" and .items[0].guild_ids == [] and ([.. | strings | select(. == $a)] | length) == 0' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM guild_rail_positions WHERE user_id = $BOB AND guild_id = $FF1")" = 0 ]
# Rejoining doesn't put it back in the folder.
tok "$TOKEN_B" call POST "/api/invites/$FINV" >/dev/null
tok "$TOKEN_B" call GET /api/users/@me/guild-layout | jq -e --arg a "$FF1" '.items[0].guild_ids == [] and .items[-1] == {"type":"guild","id":$a}' >/dev/null

step "folders: deleting a folder puts its outhouses back on the rail"
tok "$TOKEN_A" call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"Temp","guild_ids":["'"$FF1"'","'"$FF2"'"]}]}' >/dev/null
tok "$TOKEN_A" call PUT /api/users/@me/guild-layout '{"items":[]}' | jq -e --arg a "$FF1" --arg b "$FF2" --argjson n "$((NA + 1))" '
  (.items | length) == $n and all(.items[]; .type == "guild") and any(.items[]; .id == $a) and any(.items[]; .id == $b)' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM guild_folders WHERE user_id = $ALICE")" = 0 ]

# ---------------------------------------------------------------------------
# Outhouse info and Settings → Overview (feat-folders).
# ---------------------------------------------------------------------------
step "outhouse info: Manage Outhouse only, validated, logged, live, name history"
IG=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Info Hut"}' | jq -r .id)
IINV=$(tok "$TOKEN_A" call POST "/api/guilds/$IG/invites" | jq -r .code)
tok "$TOKEN_B" call POST "/api/invites/$IINV" >/dev/null
[ "$(tok "$TOKEN_B" status PATCH "/api/guilds/$IG" '{"name":"Bob was here"}')" = 403 ]
[ "$(tok "$TOKEN_C" status GET "/api/guilds/$IG/info")" = 404 ]
[ "$(tok "$TOKEN_C" status PATCH "/api/guilds/$IG" '{"name":"Carl"}')" = 404 ]
for bad in '{"name":"   "}' '{"name":"'"$(printf 'n%.0s' $(seq 1 101))"'"}' '{"tagline":"'"$(printf 't%.0s' $(seq 1 81))"'"}' \
  '{"tagline":"two\nlines"}' '{"description":"'"$(printf 'd%.0s' $(seq 1 1001))"'"}' '{"rules":"'"$(printf 'r%.0s' $(seq 1 2001))"'"}' \
  '{"links":[{"label":"Site","url":"http://example.com"}]}' '{"links":[{"label":"Site","url":"javascript:alert(1)"}]}' \
  '{"links":[{"label":"","url":"https://example.com"}]}' '{"links":[{"label":"Site","url":"https://"}]}' \
  "$(jq -nc '{links: [range(6) | {label: "L\(.)", url: "https://example.com/\(.)"}]}')"; do
  [ "$(tok "$TOKEN_A" status PATCH "/api/guilds/$IG" "$bad")" = 400 ] || { echo "accepted: $bad"; cat /tmp/body; exit 1; }
done
rm -f /tmp/gw-info.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-info.jsonl 20 & GWI=$!
wait_for /tmp/gw-info.jsonl '.t == "READY"'
tok "$TOKEN_A" call PATCH "/api/guilds/$IG" '{"name":"  Info Shed ","tagline":"Raids on Tuesdays","description":"We **raid**.\nBring food.","rules":"1. Be nice.","links":[{"label":"Website","url":"https://example.com/shed"},{"label":"","url":""}]}' \
  | jq -e '.name == "Info Shed" and .tagline == "Raids on Tuesdays" and .rules == "1. Be nice." and .links == [{"label":"Website","url":"https://example.com/shed"}] and .can_manage' >/dev/null
wait_for /tmp/gw-info.jsonl '.t == "GUILD_UPDATE" and .d.id == $g and .d.name == "Info Shed"' --arg g "$IG"
kill $GWI 2>/dev/null || true
tok "$TOKEN_A" call PATCH "/api/guilds/$IG" '{"name":"Info Hut"}' >/dev/null
tok "$TOKEN_B" call GET "/api/guilds/$IG/info" | jq -e --arg a "$ALICE" '.name == "Info Hut" and .member_count == 2 and .owner.id == $a and (.can_manage | not)
  and .joined_at != null and (.name_history | length) == 2 and .name_history[0].old == "Info Shed" and .name_history[0].new == "Info Hut"
  and .name_history[1].old == "Info Hut" and .description == "We **raid**.\nBring food."' >/dev/null
tok "$TOKEN_B" call GET /api/guilds | jq -e --arg g "$IG" '.[] | select(.id == $g) | .name == "Info Hut" and .icon_url == null' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$IG/audit-log?action=guild_update" | jq -e '(.entries | length) >= 2 and any(.entries[]; .details | test("Name: Info Hut → Info Shed"))
  and any(.actions[]; .key == "guild_update")' >/dev/null
# Manage Outhouse through a role works too.
MGR=$(tok "$TOKEN_A" call POST "/api/guilds/$IG/roles" '{"name":"Manager","permissions":"'"$((1 << 26))"'"}' | jq -r .id)
tok "$TOKEN_A" call PUT "/api/guilds/$IG/members/$BOB/roles/$MGR" >/dev/null
tok "$TOKEN_B" call PATCH "/api/guilds/$IG" '{"tagline":"Bob tidied this"}' | jq -e '.tagline == "Bob tidied this" and .can_manage' >/dev/null

# ---------------------------------------------------------------------------
# "View outhouse as role" (feat-folders): the server's preview permissions.
# ---------------------------------------------------------------------------
step "view as role: same rules as real members (overwrites, hidden and read-only channels)"
PVG=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Preview Hall"}' | jq -r .id)
PVI=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/invites" | jq -r .code)
tok "$TOKEN_B" call POST "/api/invites/$PVI" >/dev/null
PR1=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/roles" '{"name":"Raider"}' | jq -r .id)
PR2=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/roles" '{"name":"Officer"}' | jq -r .id)
PHID=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/channels" '{"name":"officers"}' | jq -r .id)
PRO=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/channels" '{"name":"announcements"}' | jq -r .id)
PRD=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/channels" '{"name":"raider-quiet"}' | jq -r .id)
PSEC=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/channels" '{"name":"owner-only"}' | jq -r .id)
tok "$TOKEN_A" call PUT "/api/channels/$PHID/permissions/$PVG" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
tok "$TOKEN_A" call PUT "/api/channels/$PHID/permissions/$PR2" '{"kind":"role","allow":"1","deny":"0"}' >/dev/null
tok "$TOKEN_A" call PUT "/api/channels/$PRO/permissions/$PVG" '{"kind":"role","allow":"0","deny":"2"}' >/dev/null
tok "$TOKEN_A" call PUT "/api/channels/$PRD/permissions/$PR1" '{"kind":"role","allow":"0","deny":"2"}' >/dev/null
tok "$TOKEN_A" call PUT "/api/channels/$PSEC/permissions/$PVG" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
bits() { jq -r --arg c "$1" '.channels[$c] // "0"' /tmp/pv.json; }
has() { [ "$(( $1 & $2 ))" != 0 ]; }
hasnt() { [ "$(( $1 & $2 ))" = 0 ]; }
tok "$TOKEN_A" call GET "/api/guilds/$PVG/preview-permissions?roles=" > /tmp/pv.json
hasnt "$(bits "$PHID")" 1; has "$(bits "$PRO")" 1; hasnt "$(bits "$PRO")" 2; has "$(bits "$PRD")" 2
jq -e '(.guild | tonumber) % 2 == 1 and .top_position == 0' /tmp/pv.json >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$PVG/preview-permissions?roles=$PR2" > /tmp/pv.json
has "$(bits "$PHID")" 1; has "$(bits "$PHID")" 2; hasnt "$(bits "$PRO")" 2
tok "$TOKEN_A" call GET "/api/guilds/$PVG/preview-permissions?roles=$PR1" > /tmp/pv.json
hasnt "$(bits "$PHID")" 1; hasnt "$(bits "$PRD")" 2; has "$(bits "$PRD")" 1
tok "$TOKEN_A" call GET "/api/guilds/$PVG/preview-permissions?roles=$PR1,$PR2,$PVG" > /tmp/pv.json
has "$(bits "$PHID")" 1; hasnt "$(bits "$PRD")" 2
# Real member with the same roles agrees.
tok "$TOKEN_A" call PUT "/api/guilds/$PVG/members/$BOB/roles/$PR1" >/dev/null
tok "$TOKEN_B" call GET "/api/guilds/$PVG/permissions" | jq -e --arg c "$PRD" --arg h "$PHID" '(.channels[$c] | tonumber) % 4 == 1 and (.channels[$h] == null or ((.channels[$h] | tonumber) % 2 == 0))' >/dev/null

step "view as role: Manage Roles only, this outhouse's roles only, never shows channels you can't see"
[ "$(tok "$TOKEN_B" status GET "/api/guilds/$PVG/preview-permissions?roles=$PR2")" = 403 ]
[ "$(tok "$TOKEN_C" status GET "/api/guilds/$PVG/preview-permissions")" = 404 ]
[ "$(tok "$TOKEN_A" status GET "/api/guilds/$PVG/preview-permissions?roles=$MGR")" = 400 ]   # a role from another outhouse
[ "$(tok "$TOKEN_A" status GET "/api/guilds/$PVG/preview-permissions?roles=abc")" = 400 ]
PMR=$(tok "$TOKEN_A" call POST "/api/guilds/$PVG/roles" '{"name":"Role manager","permissions":"'"$((1 << 25))"'"}' | jq -r .id)
tok "$TOKEN_A" call PUT "/api/guilds/$PVG/members/$BOB/roles/$PMR" >/dev/null
tok "$TOKEN_B" call GET "/api/guilds/$PVG/preview-permissions?roles=$PR2" | jq -e --arg s "$PSEC" --arg h "$PHID" '(.channels | has($s) | not) and (.channels | has($h) | not)' >/dev/null
tok "$TOKEN_A" call GET "/api/guilds/$PVG/preview-permissions?roles=$PR2" | jq -e --arg s "$PSEC" '.channels | has($s)' >/dev/null

# ---------------------------------------------------------------------------
# The site owner's Outhouses directory: search, numbers, a read-only look,
# shutting an outhouse down, restoring and deleting it.
# ---------------------------------------------------------------------------
step "outhouses directory: site owner only; counts, search (name, old name, id, owner), sort, pages"
[ "$(tok "$TOKEN_B" status GET /api/safety/outhouses)" = 403 ]
[ "$(tok "$TOKEN_B" status GET /api/safety/server-status)" = 403 ]
tok "$TOKEN_A" call GET /api/safety/server-status | jq -e '.db_ok and (.version | length > 0)' >/dev/null
DXA=$(tok "$TOKEN_A" call POST /api/guilds '{"name":"Dir Alpha"}' | jq -r .id)
tok "$TOKEN_A" call PATCH "/api/guilds/$DXA" '{"name":"Dir Gamma"}' >/dev/null
DXB=$(tok "$TOKEN_B" call POST /api/guilds '{"name":"Dir Bobs Shed"}' | jq -r .id)
DXBC=$(tok "$TOKEN_B" call GET "/api/guilds/$DXB/channels" | jq -r '[.[] | select(.kind == 0)][0].id')
DXBV=$(tok "$TOKEN_B" call GET "/api/guilds/$DXB/channels" | jq -r '[.[] | select(.kind == 2)][0].id')
DXINV=$(tok "$TOKEN_B" call POST "/api/guilds/$DXB/invites" | jq -r .code)
tok "$TOKEN_C" call POST "/api/invites/$DXINV" >/dev/null
DXM=$(tok "$TOKEN_B" call POST "/api/channels/$DXBC/messages" '{"content":"welcome to the shed"}' | jq -r .id)
tok "$TOKEN_C" call POST "/api/channels/$DXBC/messages" '{"content":"nice shed"}' >/dev/null
tok "$TOKEN_A" call GET /api/safety/outhouses > /tmp/dir.json
jq -e --argjson n "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM guilds WHERE id <> 0")" \
  --argjson p "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM users WHERE NOT is_bot")" \
  '.total_outhouses == $n and .total_people == $p and .matching == $n and .page == 1 and (.outhouses | length) == ([$n, 25] | min)' /tmp/dir.json >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=alpha" | jq -e --arg g "$DXA" '.matching == 1 and .outhouses[0].id == $g and .outhouses[0].name == "Dir Gamma" and .outhouses[0].previous_names == ["Dir Alpha"]' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=$DXB" | jq -e --arg g "$DXB" '.matching == 1 and .outhouses[0].id == $g' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=bob" | jq -e --arg g "$DXB" 'any(.outhouses[]; .id == $g and .owner.username == "bob")' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=$(enc '%')" | jq -e '.matching == 0' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=Dir&sort=members" | jq -e --arg g "$DXB" '.outhouses[0].id == $g and .outhouses[0].members == 2
  and .outhouses[0].messages_7d == 2 and .outhouses[0].messages_30d == 2 and .outhouses[0].channels >= 2 and (.outhouses[0].shut_down | not)' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=Dir&sort=created" | jq -e --arg g "$DXB" '.outhouses[0].id == $g' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=Dir&sort=name" | jq -e '[.outhouses[].name] == ["Dir Bobs Shed", "Dir Gamma"]' >/dev/null
for i in $(seq -w 1 26); do tok "$TOKEN_A" call POST /api/guilds '{"name":"Paged '"$i"'"}' >/dev/null; done
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=paged&page=1" | jq -e '.matching == 26 and .pages == 2 and (.outhouses | length) == 25 and .outhouses[0].name == "Paged 01"' >/dev/null
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=paged&page=2" | jq -e '.page == 2 and (.outhouses | length) == 1 and .outhouses[0].name == "Paged 26"' >/dev/null

step "outhouses directory: details and a read-only look (logged, without joining)"
[ "$(tok "$TOKEN_B" status GET "/api/safety/outhouses/$DXB")" = 403 ]
tok "$TOKEN_A" call GET "/api/safety/outhouses/$DXB" | jq -e --arg b "$BOB" '.name == "Dir Bobs Shed" and (.members_list | length) == 2
  and any(.members_list[]; .id == $b and .owner and .age_bracket == "adult") and any(.channels_list[]; .messages == 2) and (.reports | length) == 0' >/dev/null
[ "$(tok "$TOKEN_B" status GET "/api/safety/outhouses/$DXB/channels/$DXBC/messages")" = 403 ]
tok "$TOKEN_A" call GET "/api/safety/outhouses/$DXB/channels/$DXBC/messages" | jq -e '(.messages | length) == 2 and .messages[0].content == "welcome to the shed" and (.has_more | not)' >/dev/null
[ "$(tok "$TOKEN_A" status GET "/api/safety/outhouses/$DXA/channels/$DXBC/messages")" = 404 ]   # channel from another outhouse
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM guild_members WHERE guild_id = $DXB AND user_id = $ALICE")" = 0 ]
tok "$TOKEN_A" call GET "/api/safety/outhouses/$DXB" | jq -e 'any(.log[]; .action == "outhouse_view" and (.details | test("Looked at #")))' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"family","family":true}' | jq -e '.family and any(.log[]; .action == "family")' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"family","family":false}' | jq -e '(.family | not)' >/dev/null
[ "$(tok "$TOKEN_B" status POST "/api/safety/outhouses/$DXB/actions" '{"action":"family","family":true}')" = 403 ]

step "outhouses directory: shutting one down freezes it; members keep it (closed) for the grace period and can leave"
[ "$(report "$TOKEN_C" message "$DXM" spam "shed spam")" = 201 ]; DXR=$(report_id message "$DXM")
tok "$TOKEN_D" call POST "/api/invites/$DXINV" >/dev/null
tok "$TOKEN_C" call PUT /api/users/@me/guild-layout '{"items":[{"type":"folder","name":"Sheds","guild_ids":["'"$DXB"'"]}]}' >/dev/null
[ "$(tok "$TOKEN_A" status POST "/api/safety/outhouses/$DXB/actions" '{"action":"shut_down","reason":"  "}')" = 400 ]
[ "$(tok "$TOKEN_A" status POST "/api/safety/outhouses/$DXB/actions" '{"action":"delete","confirm_name":"Dir Bobs Shed"}')" = 400 ]   # shut it down first
rm -f /tmp/gw-shut.jsonl; node ci/gateway_probe.mjs "$BASE" "$TOKEN_C" /tmp/gw-shut.jsonl 20 & GWS=$!
wait_for /tmp/gw-shut.jsonl '.t == "READY"'
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"shut_down","reason":"Spam shed (test)"}' | jq -e '.shut_down and .shut_down_reason == "Spam shed (test)" and .members_remove_at != null and .members_removed_at == null' >/dev/null
wait_for /tmp/gw-shut.jsonl '.t == "GUILD_SHUT_DOWN" and .d.guild_id == $g and .d.name == "Dir Bobs Shed" and .d.remove_at != null' --arg g "$DXB"
kill $GWS 2>/dev/null || true
# Still on their rail, closed until the removal date (about 7 days away).
tok "$TOKEN_C" call GET /api/guilds | jq -e --arg g "$DXB" '.[] | select(.id == $g) | .closed_until != null
  and ((.closed_until | sub("\\.[0-9]+"; "") | fromdateiso8601) - now) > 6.9 * 86400' >/dev/null
tok "$TOKEN_C" call GET /api/users/@me/guild-layout | jq -e --arg g "$DXB" '.items[0].guild_ids == [$g]' >/dev/null
tok "$TOKEN_C" call GET /api/users/@me/unread >/dev/null   # a closed outhouse doesn't break unread counts
for t in "$TOKEN_B" "$TOKEN_C"; do
  [ "$(tok "$t" status GET "/api/guilds/$DXB/info")" = 410 ]; grep -q "shut down by the server owner" /tmp/body
  [ "$(tok "$t" status POST "/api/channels/$DXBC/messages" '{"content":"still here?"}')" = 410 ]
  [ "$(tok "$t" status GET "/api/guilds/$DXB/channels")" = 410 ]
done
[ "$(tok "$TOKEN_B" status POST "/api/guilds/$DXB/invites")" = 410 ]
[ "$(tok "$TOKEN_B" status POST "/api/channels/$DXBV/voice/join")" = 410 ]
[ "$(tok "$TOKEN_B" status DELETE "/api/guilds/$DXB")" = 410 ]   # the owner can't delete the evidence
[ "$(TOKEN="" status GET "/api/invites/$DXINV")" = 410 ]
tok "$TOKEN_A" call GET "/api/safety/outhouses?q=shed" | jq -e '.outhouses[0].shut_down and .total_shut_down >= 1' >/dev/null
tok "$TOKEN_A" call GET /api/safety/digest | jq -e 'any(.outhouses_shut_down[]; test("Dir Bobs Shed"))' >/dev/null
# Members heard about it from Outhouse Safety, with the removal date.
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM messages m JOIN dm_participants r ON r.channel_id = m.channel_id WHERE r.user_id = (SELECT id FROM users WHERE username = 'carl') AND m.content LIKE '%Dir Bobs Shed%has been shut down by the server owner%removed from your list on%'")" -ge 1 ]
# The owner can still look (evidence).
tok "$TOKEN_A" call GET "/api/safety/outhouses/$DXB/channels/$DXBC/messages" | jq -e '(.messages | length) == 2' >/dev/null
# Leave now: Dora goes early.
tok "$TOKEN_D" call POST "/api/guilds/$DXB/leave" >/dev/null
tok "$TOKEN_D" call GET /api/guilds | jq -e --arg g "$DXB" 'all(.[]; .id != $g)' >/dev/null

step "outhouses directory: deleting needs the exact name and is blocked by open reports; restore within the grace period"
[ "$(tok "$TOKEN_A" status POST "/api/safety/outhouses/$DXB/actions" '{"action":"delete","confirm_name":"Dir Bobs"}')" = 400 ]
[ "$(tok "$TOKEN_A" status POST "/api/safety/outhouses/$DXB/actions" '{"action":"delete","confirm_name":"Dir Bobs Shed"}')" = 409 ]; grep -q "open report" /tmp/body
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"restore"}' | jq -e '(.shut_down | not) and .members == 2' >/dev/null
tok "$TOKEN_C" call GET /api/guilds | jq -e --arg g "$DXB" '.[] | select(.id == $g) | .closed_until == null' >/dev/null
tok "$TOKEN_C" call POST "/api/channels/$DXBC/messages" '{"content":"we are back"}' >/dev/null

step "outhouses directory: after the grace period members are removed (snapshot kept); restore re-adds them, not suspended ones"
PAINTER=$(tok "$TOKEN_B" call POST "/api/guilds/$DXB/roles" '{"name":"Painter"}' | jq -r .id)
tok "$TOKEN_B" call PUT "/api/guilds/$DXB/members/$CARL/roles/$PAINTER" >/dev/null
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"shut_down","reason":"Again","suspend_owner":true}' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT suspended_until > now() AND suspension_pending FROM users WHERE id = $BOB")" = t ]
[ "$(TOKEN="" status POST "/api/safety/outhouses/$DXB/remove-members-now")" = 401 ]
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/remove-members-now" | jq -e '.removed == 2' >/dev/null
tok "$TOKEN_C" call GET /api/guilds | jq -e --arg g "$DXB" 'all(.[]; .id != $g)' >/dev/null
tok "$TOKEN_C" call GET /api/users/@me/guild-layout | jq -e '.items[0].name == "Sheds" and .items[0].guild_ids == []' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM guild_rail_positions WHERE guild_id = $DXB")" = 0 ]
tok "$TOKEN_A" call GET "/api/safety/outhouses/$DXB" | jq -e --arg c "$CARL" '.members == 0 and .members_removed_at != null and .removed_members == 2
  and any(.member_snapshot[]; .user_id == $c and .roles == ["Painter"] and .age_bracket == "adult" and .joined_at != null)
  and any(.log[]; .action == "outhouse_members_removed")' >/dev/null
[ "$(tok "$TOKEN_A" status POST "/api/safety/outhouses/$DXB/remove-members-now")" = 400 ]
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"restore"}' | jq -e --arg c "$CARL" '(.shut_down | not) and .members == 1
  and .members_list[0].id == $c and .members_list[0].roles == ["Painter"] and any(.log[]; .action == "outhouse_restore" and (.details | test("1 previous member added back, 1 skipped")))' >/dev/null
tok "$TOKEN_C" call GET /api/guilds | jq -e --arg g "$DXB" 'any(.[]; .id == $g)' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/users/$BOB/actions" '{"action":"unsuspend"}' >/dev/null
TOKEN_B=$(TOKEN="" call POST /api/auth/login '{"username":"bob","password":"password123"}' | jq -r .token)

step "outhouses directory: delete for good once its reports are closed"
act "$DXR" '{"action":"dismiss","text":"not spam"}' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"shut_down","reason":"Closing for good"}' >/dev/null
tok "$TOKEN_A" call POST "/api/safety/outhouses/$DXB/actions" '{"action":"delete","confirm_name":"Dir Bobs Shed"}' | jq -e '.deleted' >/dev/null
[ "$(tok "$TOKEN_A" status GET "/api/safety/outhouses/$DXB")" = 404 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE guild_id = $DXB AND action IN ('outhouse_shut_down','outhouse_restore','outhouse_delete','outhouse_view','outhouse_members_removed')")" -ge 8 ]

# ---------------------------------------------------------------------------
# Email, password resets and two-factor sign-in (feat-auth): its own script,
# so it can also run on its own against a local server.
# ---------------------------------------------------------------------------
step "email, password resets, two-factor (ci/auth_test.sh)"
BASE=$BASE MAIL_DIR=$MAIL_DIR OWNER_TOKEN=$TOKEN_A bash ci/auth_test.sh

echo "Smoke test passed."
