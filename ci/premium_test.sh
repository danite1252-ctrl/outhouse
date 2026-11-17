#!/usr/bin/env bash
# Premium (api/premium.rs): Golden Throne (a person) and Golden Outhouse (an
# outhouse). Grants, revokes and expiry; the owner-only `grant_premium`
# permission; the `premium` flags in payloads; every gated feature refused
# without premium and working with it; the free Golden Outhouse slot and its
# cooldown; vanity invite links (validation, joining, the 30-day hold); and
# the owner's switches. ci/smoke.sh runs it near the end; it can also run on
# its own against a local server:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=... OWNER_TOKEN=<site owner's token> \
#   MAIL_DIR=/tmp/outhouse-mail bash ci/premium_test.sh
#
# The server needs OUTHOUSE_TEST_HOOKS=1, PREMIUM_UPLOAD_MB=30 and
# PREMIUM_PERSONAL_SOUNDS=3 (smoke.sh sets them), LiveKit settings for voice,
# and for the staff part SMTP_* pointing at ci/smtp_sink.py (MAIL_DIR) and
# AUTH_SECRET_KEY.
set -euo pipefail
set -E
trap 'echo "Premium test failed at line $LINENO" >&2' ERR

BASE=${BASE:-http://127.0.0.1:8080}
MAIL_DIR=${MAIL_DIR:-}
SB=/tmp/premium-body
RUN=${PREMIUM_RUN:-$(date +%s | tail -c 6)}
P=/tmp/premium-files
mkdir -p "$P"
pstep() { echo "== premium: $*"; }

# sreq METHOD PATH [JSON] [TOKEN] -> HTTP status (body in $SB)
sreq() {
  local method=$1 path=$2 body=${3:-} token=${4:-}
  local args=(-sS -X "$method" -o "$SB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
# expect STATUS METHOD PATH [JSON] [TOKEN]
expect() {
  local want=$1; shift
  local got; got=$(sreq "$@")
  if [ "$got" != "$want" ]; then echo "EXPECTED $want, GOT $got: $1 $2" >&2; cat "$SB" >&2; echo >&2; return 1; fi
}
# form STATUS TOKEN PATH -F... -> multipart upload
form() {
  local want=$1 token=$2 path=$3; shift 3
  local got; got=$(curl -sS -o "$SB" -w "%{http_code}" -H "Authorization: Bearer $token" "$@" "$BASE$path")
  if [ "$got" != "$want" ]; then echo "EXPECTED $want, GOT $got: upload $path" >&2; head -c 600 "$SB" >&2; echo >&2; return 1; fi
}
ok() { jq -e "$@" "$SB" >/dev/null || { echo "unexpected reply:" >&2; head -c 2000 "$SB" >&2; echo >&2; return 1; }; }
q() { psql "$DATABASE_URL" -tAc "$1"; }
xff() { echo "198.18.71.$((RANDOM % 200 + 1))"; }
new_user() {
  local got; got=$(curl -sS -o "$SB" -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: $(xff)" \
    -d "{\"username\":\"$1\",\"email\":\"$1@example.test\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\",\"display_name\":\"$1\"}" \
    "$BASE/api/auth/register")
  [ "$got" = 200 ] || { echo "register $1 -> $got" >&2; cat "$SB" >&2; return 1; }
  jq -r .token "$SB"
}
uid_of() { q "SELECT id FROM users WHERE username = '$1'"; }
sweep() { expect 200 POST /api/premium/sweep "" "$OWNER_T"; }

OWNER_T=${OWNER_TOKEN:?OWNER_TOKEN is needed}

python3 - "$P" <<'PY'
import struct, sys
d = sys.argv[1]
def gif(frames):
    g = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff"
    g += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    for _ in range(frames):
        g += b"\x21\xf9\x04\x00\x0a\x00\x00\x00"
        g += b"\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00"
    return g + b"\x3b"
open(f"{d}/anim.gif", "wb").write(gif(4))
open(f"{d}/still.gif", "wb").write(gif(1))
def wav(path, seconds, rate=8000):
    n = int(rate * seconds)
    hdr = b"RIFF" + struct.pack("<I", 36 + n) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate, 1, 8) + b"data" + struct.pack("<I", n)
    open(path, "wb").write(hdr + bytes([128]) * n)
wav(f"{d}/one.wav", 1)
wav(f"{d}/eleven.wav", 11)
PY
MB=$((1024 * 1024))
truncate -s $((26 * MB)) "$P/26mb.bin"
truncate -s $((30 * MB)) "$P/30mb.bin"
truncate -s $((30 * MB + 1)) "$P/30mb1.bin"

pstep "people and outhouses"
GT=$(new_user "gt$RUN"); GT_ID=$(uid_of "gt$RUN")
PL=$(new_user "pl$RUN"); PL_ID=$(uid_of "pl$RUN")
NB=$(new_user "nb$RUN"); NB_ID=$(uid_of "nb$RUN")
expect 200 POST /api/guilds '{"name":"Golden Loo"}' "$GT"; GA=$(jq -r .id "$SB")
expect 200 POST /api/guilds '{"name":"Plain Loo"}' "$PL"; GB=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$GA/channels" '{"name":"general"}' "$GT"; CA=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$GA/channels" '{"name":"Voice","kind":2}' "$GT"; VA=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$GB/channels" '{"name":"general"}' "$PL"; CB=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$GB/channels" '{"name":"Voice","kind":2}' "$PL"; VB=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$GA/invites" '{"max_age_secs":0}' "$GT"; IA=$(jq -r .code "$SB")
expect 200 POST "/api/guilds/$GB/invites" '{"max_age_secs":0}' "$PL"; IB=$(jq -r .code "$SB")
expect 200 POST "/api/invites/$IA" "" "$PL"
expect 200 POST "/api/invites/$IB" "" "$GT"
expect 200 POST /api/dms "{\"user_ids\":[\"$PL_ID\"]}" "$GT"; DM=$(jq -r .id "$SB")
# An emoji in Plain Loo (Golden Loo has none).
form 200 "$PL" "/api/guilds/$GB/emojis" -F name=plainpog -F "file=@$P/still.gif;type=image/gif"
EMO=$(jq -r .id "$SB"); TOKE="<:plainpog:$EMO>"

pstep "payload flags start off; perks are listed"
expect 200 GET /api/users/@me "" "$GT"; ok '.premium == false'
expect 200 GET /api/guilds "" "$GT"; ok 'length == 2 and all(.[]; .premium == false)'
expect 200 GET "/api/guilds/$GA/members" "" "$GT"; ok 'all(.[]; .premium == false)'
expect 200 GET /api/users/@me/premium "" "$GT"
ok '.active == false and .slot == null and .theme == "ember" and .perks.upload_mb == 30 and .perks.free_upload_mb == 25
    and .perks.message_chars == 2000 and .perks.premium_message_chars == 4000 and .perks.personal_sounds == 3
    and .perks.emoji_premium == true and .perks.animated_premium == false
    and any(.perks.themes[]; .key == "contrast" and .free) and any(.perks.themes[]; .key == "throne" and (.free | not))'
expect 200 GET "/api/guilds/$GA/premium" "" "$GT"; ok '.active == false and .vanity == null and .allow_personal_sounds == true and .can_manage == true'
expect 200 GET "/api/guilds/$GA/premium" "" "$PL"; ok '.can_manage == false'
expect 404 GET "/api/guilds/$GA/premium" "" "$NB"   # not a member

pstep "grant_premium is the owner's (no tier has it)"
expect 403 POST "/api/safety/users/$GT_ID/premium" '{"duration":"month"}' "$PL"
expect 403 POST "/api/safety/outhouses/$GA/premium" '{"duration":"month"}' "$PL"
expect 403 GET /api/safety/premium "" "$PL"
expect 403 PUT /api/safety/premium-settings '{"emoji_premium":false}' "$PL"
expect 200 GET /api/safety/staff "" "$OWNER_T"
ok 'any(.catalogue.perms[]; .key == "grant_premium") and all(.catalogue.tiers[]; (.perms | index("grant_premium")) == null)'
expect 400 POST "/api/safety/users/$GT_ID/premium" '{"duration":"custom","ends_at":"2001-01-01T00:00:00Z"}' "$OWNER_T"
expect 400 POST "/api/safety/users/$GT_ID/premium" '{"duration":"fortnight"}' "$OWNER_T"
expect 400 POST "/api/safety/users/$GT_ID/premium/revoke" '{}' "$OWNER_T"   # nothing to revoke

if [ -n "$MAIL_DIR" ] && [ -d "$MAIL_DIR" ]; then
  pstep "staff: without grant_premium refused, with it allowed (not the owner's switches)"
  mail_n() { find "$MAIL_DIR" -name '*.json' 2>/dev/null | wc -l; }
  wait_mail() { # wait_mail TO SUBJECT_PART SINCE -> file
    local to=$1 subj=$2 since=$3 f n
    for _ in $(seq 1 80); do
      for f in $(find "$MAIL_DIR" -name '*.json' 2>/dev/null | sort); do
        n=$(basename "$f" .json); n=$((10#$n))
        [ "$n" -gt "$since" ] || continue
        if jq -e --arg to "$to" --arg s "$subj" '(.to | index($to)) and (.subject | contains($s))' "$f" >/dev/null; then
          echo "$f"; return 0
        fi
      done
      sleep 0.25
    done
    echo "no mail to $to about \"$subj\"" >&2; return 1
  }
  staff_user() { # staff_user NAME PERMS_JSON -> token
    local m0; m0=$(mail_n)
    local t; t=$(new_user "$1")
    local f; f=$(wait_mail "$1@example.test" "Confirm your email" "$m0")
    local tok; tok=$(jq -r .text "$f" | grep -oE 'token=[0-9a-f]{64}' | head -1 | cut -d= -f2)
    expect 200 POST /api/auth/verify-email "{\"token\":\"$tok\"}" >/dev/null
    expect 200 POST /api/auth/2fa/setup '{"password":"password123"}' "$t" >/dev/null
    local s; s=$(jq -r .secret "$SB")
    expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$(python3 ci/totp.py "$s" -1)\"}" "$t" >/dev/null
    expect 200 POST /api/safety/staff "{\"username\":\"$1\",\"tier\":\"custom\",\"perms\":$2}" "$OWNER_T"
    echo "$t"
  }
  S1=$(staff_user "psup$RUN" '["people"]')
  S2=$(staff_user "pgra$RUN" '["people","grant_premium"]')
  expect 403 POST "/api/safety/users/$NB_ID/premium" '{"duration":"month"}' "$S1"
  expect 403 GET /api/safety/premium "" "$S1"
  expect 200 GET /api/safety/premium "" "$S2"; ok '.owner == false'
  expect 403 PUT /api/safety/premium-settings '{"emoji_premium":false}' "$S2"
  expect 200 POST "/api/safety/users/$NB_ID/premium" '{"duration":"year","note":"staff grant"}' "$S2"; ok '.active == true'
  [ "$(q "SELECT count(*) FROM safety_log WHERE action = 'premium_grant' AND target_user_id = $NB_ID AND staff_perm = 'grant_premium'")" = 1 ]
  expect 200 POST "/api/safety/users/$NB_ID/premium/revoke" '{}' "$S2"; ok '.active == false'
fi

pstep "without premium: every gated thing is refused"
expect 403 PUT /api/users/@me/theme '{"theme":"throne"}' "$GT"
expect 400 PUT /api/users/@me/theme '{"theme":"nope"}' "$GT"
expect 200 PUT /api/users/@me/theme '{"theme":"contrast"}' "$GT"; ok '.theme == "contrast" and .theme_in_use == "contrast"'  # accessibility is free
expect 200 PUT /api/users/@me/theme '{"theme":"ember"}' "$GT"
expect 403 PUT /api/users/@me/profile-gradient '{"colors":[16711680,255]}' "$GT"
expect 200 PUT /api/users/@me/profile-gradient '{"colors":null}' "$GT"
form 403 "$GT" /api/users/@me/personal-sounds -F name=Mine -F "file=@$P/one.wav"
expect 200 GET /api/users/@me/personal-sounds "" "$GT"; ok '.premium == false and .max == 3 and (.sounds | length) == 0'
expect 403 PUT /api/users/@me/premium-entrance '{"sound_id":"personal:1"}' "$GT"
expect 403 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GA\"}" "$GT"
expect 403 PUT "/api/guilds/$GA/vanity" '{"code":"golden-loo"}' "$GT"
# 2000 characters free.
LONG2000=$(python3 -c 'print("a" * 2000)'); LONG2001="${LONG2000}b"
LONG4000=$(python3 -c 'print("c" * 4000)'); LONG4001="${LONG4000}d"
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG2000\"}" "$GT"
expect 400 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG2001\"}" "$GT"; grep -q "2000" "$SB"
expect 200 POST "/api/channels/$CA/messages" '{"content":"short"}' "$GT"; MSG=$(jq -r .id "$SB")
expect 400 PATCH "/api/channels/$CA/messages/$MSG" "{\"content\":\"$LONG2001\"}" "$GT"
# Other outhouses' emoji: text, not the emoji; their own outhouse's work.
expect 200 POST "/api/channels/$CA/messages" '{"content":"hi :plainpog:"}' "$GT"; ok '.content == "hi :plainpog:"'
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"raw $TOKE\"}" "$GT"; ok '.content == "raw :plainpog:"'
expect 200 POST "/api/channels/$CA/messages" '{"content":"bogus <:fake:123>"}' "$GT"; ok '.content == "bogus :fake:"'
expect 200 POST "/api/channels/$DM/messages" '{"content":":plainpog:"}' "$GT"; ok '.content == ":plainpog:"'
expect 200 POST "/api/channels/$CB/messages" '{"content":":plainpog:"}' "$GT"; ok --arg e "$TOKE" '.content == $e'
expect 403 PUT "/api/channels/$CA/messages/$MSG/reactions/$(jq -rn --arg v "$TOKE" '$v|@uri')" "" "$GT"
# Someone who can (Plain Loo's owner is in Golden Loo too... but has no Golden Throne either):
expect 403 PUT "/api/channels/$CA/messages/$MSG/reactions/$(jq -rn --arg v "$TOKE" '$v|@uri')" "" "$PL"
# 25 MB files: refused before reading (413) in a plain outhouse and plain DMs.
form 413 "$GT" "/api/channels/$CA/drafts" -F "file=@$P/26mb.bin"; grep -q "25 MB" "$SB"
form 413 "$GT" "/api/channels/$DM/drafts" -F "file=@$P/26mb.bin"
form 413 "$GT" "/api/channels/$CA/messages/upload" -F "content=big" -F "files=@$P/26mb.bin"
# Animated pictures are free by default: the link says it moves; ?still=1 is its first frame.
form 200 "$GT" /api/users/@me/avatar -F "file=@$P/anim.gif;type=image/gif"
AV=$(jq -r .avatar_url "$SB"); case "$AV" in *anim=1*) ;; *) echo "animated avatar link: $AV" >&2; exit 1;; esac
curl -fsS "$BASE${AV%%\?*}" -o "$P/av.gif"; cmp -s "$P/av.gif" "$P/anim.gif"
curl -fsS "$BASE${AV%%\?*}?still=1" -o "$P/av-still.gif"
[ "$(stat -c %s "$P/av-still.gif")" -lt "$(stat -c %s "$P/anim.gif")" ] && head -c 6 "$P/av-still.gif" | grep -q GIF89a
form 200 "$PL" "/api/guilds/$GB/icon" -F "file=@$P/anim.gif;type=image/gif"
expect 200 GET /api/guilds "" "$PL"; ok --arg g "$GB" 'any(.[]; .id == $g and (.icon_url | test("anim=1")))'
# The owner's switch makes them premium: not-premium people and outhouses show the first frame.
expect 200 PUT /api/safety/premium-settings '{"animated_premium":true}' "$OWNER_T"; ok '.animated_premium == true'
expect 200 GET /api/users/@me "" "$GT"; ok '.avatar_url | test("still=1")'
expect 200 GET /api/guilds "" "$PL"; ok --arg g "$GB" 'any(.[]; .id == $g and (.icon_url | test("still=1")))'

pstep "grant Golden Throne (a month) and Golden Outhouse (forever)"
expect 200 POST "/api/safety/users/$GT_ID/premium" '{"duration":"month","note":"for testing"}' "$OWNER_T"; ok '.active == true'
expect 200 POST "/api/safety/outhouses/$GB/premium" '{"duration":"forever","note":"family"}' "$OWNER_T"; ok '.active == true'
expect 200 GET /api/users/@me "" "$GT"; ok '.premium == true and (.avatar_url | test("anim=1"))'
expect 200 GET /api/users/@me/premium "" "$GT"
ok '.active == true and .forever == false and (.until | sub("\\.[0-9]+"; "") | fromdateiso8601) > (now + 29*86400) and .entitlements[0].note == "for testing"'
expect 200 GET "/api/guilds/$GA/members" "" "$PL"; ok --arg u "$GT_ID" 'any(.[]; .id == $u and .premium)'
expect 200 GET "/api/profiles/$GT_ID" "" "$PL"; ok '.premium == true'
expect 200 GET "/api/users/$GT_ID/profile/full" "" "$PL"; ok '.premium == true'
expect 200 GET /api/guilds "" "$PL"; ok --arg a "$GA" --arg b "$GB" 'any(.[]; .id == $b and .premium and (.icon_url | test("anim=1"))) and any(.[]; .id == $a and (.premium | not))'
expect 200 GET "/api/guilds/$GB/premium" "" "$PL"; ok '.active == true and .forever == true and .sources[0].source == "grant"'
expect 200 GET "/api/guilds/$GB/info" "" "$PL"; ok '.premium == true'
expect 200 PUT /api/safety/premium-settings '{"reset":true}' "$OWNER_T"; ok '.animated_premium == false'
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'premium_grant' AND (target_user_id = $GT_ID OR guild_id = $GB)")" = 2 ]

pstep "with Golden Throne: themes, profile colours, longer messages, emoji everywhere, DM uploads"
expect 200 PUT /api/users/@me/theme '{"theme":"throne"}' "$GT"; ok '.theme == "throne" and .theme_in_use == "throne"'
expect 200 PUT /api/users/@me/profile-gradient '{"colors":[16753920,8388736]}' "$GT"
expect 400 PUT /api/users/@me/profile-gradient '{"colors":[1]}' "$GT"
expect 200 GET "/api/profiles/$GT_ID" "" "$PL"; ok '.profile_gradient == [16753920,8388736]'
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG4000\"}" "$GT"
expect 400 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG4001\"}" "$GT"
expect 200 PATCH "/api/channels/$CA/messages/$MSG" "{\"content\":\"$LONG2001\"}" "$GT"
expect 400 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG2001\"}" "$PL"   # still 2000 for others
expect 200 POST "/api/channels/$CA/messages" '{"content":"hi :plainpog:"}' "$GT"; ok --arg e "$TOKE" '.content == ("hi " + $e)'
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"raw $TOKE\"}" "$GT"; ok --arg e "$TOKE" '.content == ("raw " + $e)'
expect 200 POST "/api/channels/$DM/messages" '{"content":":plainpog:"}' "$GT"; ok --arg e "$TOKE" '.content == $e'
expect 204 PUT "/api/channels/$CA/messages/$MSG/reactions/$(jq -rn --arg v "$TOKE" '$v|@uri')" "" "$GT"
# ...and once it's on the message, others can add to it.
expect 204 PUT "/api/channels/$CA/messages/$MSG/reactions/$(jq -rn --arg v "$TOKE" '$v|@uri')" "" "$PL"
# Someone not in Plain Loo still sees the picture (public by key).
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/emojis/$EMO")" = 200 ]
# Emoji from an outhouse they aren't in stay text even with Golden Throne.
expect 200 POST /api/guilds '{"name":"Far Loo"}' "$NB"; GF=$(jq -r .id "$SB")
form 200 "$NB" "/api/guilds/$GF/emojis" -F name=farpog -F "file=@$P/still.gif;type=image/gif"; FEMO=$(jq -r .id "$SB")
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"<:farpog:$FEMO>\"}" "$GT"; ok '.content == ":farpog:"'
# 100 MB (30 here) in DMs for Golden Throne senders; the other person's DMs stay 25.
form 200 "$GT" "/api/channels/$DM/drafts" -F "file=@$P/26mb.bin"; D1=$(jq -r .draft_id "$SB")
form 200 "$GT" "/api/channels/$DM/drafts" -F "file=@$P/30mb.bin"; D2=$(jq -r .draft_id "$SB")
form 413 "$GT" "/api/channels/$DM/drafts" -F "file=@$P/30mb1.bin"; grep -q "30 MB" "$SB"
form 413 "$PL" "/api/channels/$DM/drafts" -F "file=@$P/26mb.bin"
expect 204 DELETE "/api/drafts/$D1" "" "$GT"; expect 204 DELETE "/api/drafts/$D2" "" "$GT"
# But not in a plain outhouse (that's Golden Outhouse's).
form 413 "$GT" "/api/channels/$CA/drafts" -F "file=@$P/26mb.bin"

pstep "Golden Outhouse: bigger uploads in its channels"
form 200 "$PL" "/api/channels/$CB/drafts" -F "file=@$P/30mb.bin"; D3=$(jq -r .draft_id "$SB")
form 413 "$PL" "/api/channels/$CB/drafts" -F "file=@$P/30mb1.bin"; grep -q "30 MB" "$SB"
form 200 "$PL" "/api/channels/$CB/messages/upload" -F "content=big file" -F "files=@$P/26mb.bin"
ok '.attachments[0].size == 27262976'
expect 204 DELETE "/api/drafts/$D3" "" "$PL"

pstep "personal soundboard and entrance sound"
form 200 "$GT" /api/users/@me/personal-sounds -F name=Trumpet -F "emoji=🎺" -F "file=@$P/one.wav"; PS1=$(jq -r .id "$SB")
ok '.personal == true and .locked == false and (.id | startswith("personal:"))'
form 400 "$GT" /api/users/@me/personal-sounds -F name=Long -F "file=@$P/eleven.wav"
form 200 "$GT" /api/users/@me/personal-sounds -F name=Two -F "file=@$P/one.wav"
form 200 "$GT" /api/users/@me/personal-sounds -F name=Three -F "file=@$P/one.wav"; PS3=$(jq -r .id "$SB")
form 400 "$GT" /api/users/@me/personal-sounds -F name=Four -F "file=@$P/one.wav"   # the limit (3 here)
expect 200 PATCH "/api/users/@me/personal-sounds/$PS3" '{"name":"Third"}' "$GT"; ok '.name == "Third"'
expect 204 DELETE "/api/users/@me/personal-sounds/$PS3" "" "$GT"
expect 404 PATCH "/api/users/@me/personal-sounds/$PS1" '{"name":"Mine now"}' "$PL"   # only theirs
expect 200 PUT /api/users/@me/premium-entrance "{\"sound_id\":\"$PS1\"}" "$GT"; ok --arg s "$PS1" '.entrance_id == $s and (.sounds | length) == 2'
PURL=$(jq -r --arg s "$PS1" '.sounds[] | select(.id == $s) | .url' "$SB")
[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE$PURL")" = 200 ]
# Plain Loo's owner hears it in Golden Loo's voice channel.
expect 200 POST "/api/channels/$VA/voice/join" "" "$PL"
rm -f /tmp/gw-premium.jsonl
node ci/gateway_probe.mjs "$BASE" "$PL" /tmp/gw-premium.jsonl 25 &
PROBE=$!
for _ in $(seq 1 40); do grep -q '"READY"' /tmp/gw-premium.jsonl 2>/dev/null && break; sleep 0.25; done
expect 200 POST "/api/channels/$VA/voice/join" "" "$GT"
sleep 0.5
expect 204 POST "/api/sounds/$PS1/play" "" "$GT"
expect 404 POST "/api/sounds/$PS1/play" "" "$PL"   # not theirs
for _ in $(seq 1 40); do grep -q 'SOUNDBOARD_PLAY' /tmp/gw-premium.jsonl && break; sleep 0.25; done
kill $PROBE 2>/dev/null || true
jq -s -e --arg u "$PURL" --arg s "$PS1" '
  any(.[]; .t == "ENTRANCE_SOUND" and .d.url == $u)
  and any(.[]; .t == "SOUNDBOARD_PLAY" and .d.sound_id == $s and .d.url == $u)' /tmp/gw-premium.jsonl >/dev/null \
  || { echo "personal sound events wrong:" >&2; cat /tmp/gw-premium.jsonl >&2; exit 1; }
expect 204 POST /api/voice/leave "" "$PL"
# Plain Loo turns personal sounds off (Manage Outhouse only).
expect 403 PUT "/api/guilds/$GB/premium-settings" '{"allow_personal_sounds":false}' "$GT"
expect 200 PUT "/api/guilds/$GB/premium-settings" '{"allow_personal_sounds":false}' "$PL"; ok '.allow_personal_sounds == false'
expect 200 POST "/api/channels/$VB/voice/join" "" "$GT"
sleep 2.2
expect 403 POST "/api/sounds/$PS1/play" "" "$GT"; grep -q "turned off" "$SB"
expect 204 POST /api/voice/leave "" "$GT"
expect 200 PUT "/api/guilds/$GB/premium-settings" '{"allow_personal_sounds":true}' "$PL"
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GB AND details LIKE '%personal soundboard%'")" = 2 ]

pstep "the free Golden Outhouse slot (one, moved once a week)"
expect 404 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GF\"}" "$GT"   # not a member there
expect 200 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GA\"}" "$GT"; ok --arg g "$GA" '.slot.guild_id == $g and .slot.can_move_at != null'
expect 200 GET /api/guilds "" "$PL"; ok --arg g "$GA" 'any(.[]; .id == $g and .premium)'
expect 200 GET "/api/guilds/$GA/premium" "" "$GT"; ok '.active == true and .sources[0].source == "slot" and .my_slot_here == true'
expect 409 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GB\"}" "$GT"; grep -q "7 days" "$SB"
expect 200 DELETE /api/users/@me/premium/slot "" "$GT"; ok '.slot.guild_id == null'
expect 200 GET "/api/guilds/$GA/premium" "" "$GT"; ok '.active == false'
expect 409 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GA\"}" "$GT"
q "UPDATE premium_slots SET applied_at = now() - interval '8 days' WHERE user_id = $GT_ID" >/dev/null
expect 200 PUT /api/users/@me/premium/slot "{\"guild_id\":\"$GA\"}" "$GT"
expect 200 GET "/api/guilds/$GA/premium" "" "$GT"; ok '.active == true'
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GA AND action = 'premium_slot_add'")" = 2 ]

pstep "vanity invite links: validation"
expect 403 PUT "/api/guilds/$GA/vanity" '{"code":"golden-loo"}' "$PL"   # needs Manage Outhouse
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"ab"}' "$GT"
expect 400 PUT "/api/guilds/$GA/vanity" "{\"code\":\"$(python3 -c 'print("a"*33)')\"}" "$GT"
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"-loo"}' "$GT"
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"loo-"}' "$GT"
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"golden_loo"}' "$GT"
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"Admin"}' "$GT"; grep -q "reserved" "$SB"
expect 400 PUT "/api/guilds/$GA/vanity" '{"code":"every-one"}' "$GT"
expect 200 POST /api/safety/username-words "{\"word\":\"grotty$RUN\"}" "$OWNER_T"
expect 400 PUT "/api/guilds/$GA/vanity" "{\"code\":\"my-gr0tty$RUN\"}" "$GT"; grep -q "isn't allowed" "$SB"
expect 200 DELETE /api/safety/username-words "{\"word\":\"grotty$RUN\"}" "$OWNER_T"
LC=$(tr '[:upper:]' '[:lower:]' <<<"$IB")
expect 409 PUT "/api/guilds/$GA/vanity" "{\"code\":\"$LC\"}" "$GT"   # an invite code (any case)
expect 200 PUT "/api/guilds/$GA/vanity" "{\"code\":\"Golden-Loo$RUN\"}" "$GT"
VC="golden-loo$RUN"; ok --arg c "$VC" '.vanity.code == $c and .vanity.active == true and (.vanity.url | endswith("/invite/" + $c))'
expect 409 PUT "/api/guilds/$GB/vanity" "{\"code\":\"GOLDEN-loo$RUN\"}" "$PL"   # taken (any case)
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GA AND action = 'vanity_set'")" = 1 ]

pstep "vanity invite links: joining works like an invite"
expect 200 GET "/api/invites/$VC" ""; ok --arg g "$GA" '.guild.id == $g and .vanity == true and .guild.premium == true and .expires_at == null'
expect 200 GET "/api/invites/GOLDEN-LOO$RUN" ""; ok --arg g "$GA" '.guild.id == $g'
expect 200 POST "/api/invites/$VC" "" "$NB"; ok --arg g "$GA" '.id == $g'
[ "$(q "SELECT vanity_uses FROM guilds WHERE id = $GA")" = 1 ]
expect 200 POST "/api/invites/$VC" "" "$NB"   # already in: no extra use
[ "$(q "SELECT vanity_uses FROM guilds WHERE id = $GA")" = 1 ]
expect 400 DELETE "/api/invites/$VC" "" "$GT"   # it's not a made invite
BN=$(new_user "bn$RUN"); BN_ID=$(uid_of "bn$RUN")
expect 200 POST "/api/invites/$IA" "" "$BN"
expect 204 PUT "/api/guilds/$GA/bans/$BN_ID" '{}' "$GT"
expect 400 POST "/api/invites/$VC" "" "$BN"; grep -q "banned" "$SB"
PAGE=$(curl -fsS "$BASE/invite/$VC"); grep -q 'data-state="valid"' <<<"$PAGE"; grep -q "Golden Loo" <<<"$PAGE"
grep -q '"premium":true' <<<"$PAGE"

pstep "lapse: revoking Golden Throne takes the slot, pauses the vanity link and the perks"
expect 200 POST "/api/safety/users/$GT_ID/premium/revoke" '{}' "$OWNER_T"; ok '.active == false'
sweep
expect 200 GET /api/users/@me/premium "" "$GT"; ok '.active == false and .slot.guild_id == null and .theme == "throne" and .theme_in_use == "ember"'
expect 200 GET "/api/profiles/$GT_ID" "" "$PL"; ok '.premium == false and .profile_gradient == null'
expect 200 GET /api/users/@me/personal-sounds "" "$GT"; ok '.premium == false and (.sounds | length) == 2 and all(.sounds[]; .locked)'
expect 400 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG2001\"}" "$GT"
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"raw $TOKE\"}" "$GT"; ok '.content == "raw :plainpog:"'
# Old messages keep their emoji.
expect 200 GET "/api/channels/$CA/messages?limit=50" "" "$PL"; ok --arg e "$TOKE" 'any(.[]; .content == ("hi " + $e))'
expect 200 GET "/api/guilds/$GA/premium" "" "$GT"; ok '.active == false and .vanity.active == false and .vanity.paused_at != null and .vanity.release_at != null'
expect 410 GET "/api/invites/$VC" ""; grep -q "isn't active right now" "$SB"
expect 410 POST "/api/invites/$VC" "" "$BN"
PAGE=$(curl -sS "$BASE/invite/$VC"); grep -q 'data-state="paused"' <<<"$PAGE"
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GA AND action = 'vanity_paused'")" = 1 ]
form 413 "$GT" "/api/channels/$DM/drafts" -F "file=@$P/26mb.bin"
expect 403 PUT /api/users/@me/premium-entrance "{\"sound_id\":\"$PS1\"}" "$GT"
# Held 30 days for Golden Loo: Plain Loo (premium) can't take it.
expect 409 PUT "/api/guilds/$GB/vanity" "{\"code\":\"$VC\"}" "$PL"

pstep "expiry: a grant that runs out ends by itself (and is logged once)"
END=$(date -u -d '+3 seconds' +%Y-%m-%dT%H:%M:%SZ)
expect 200 POST "/api/safety/users/$GT_ID/premium" "{\"duration\":\"custom\",\"ends_at\":\"$END\"}" "$OWNER_T"; ok '.active == true'
expect 200 GET /api/users/@me "" "$GT"; ok '.premium == true'
sleep 4
expect 200 GET /api/users/@me "" "$GT"; ok '.premium == false'
sweep; ok '.ended >= 1'
sweep; ok '.ended == 0'
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'premium_expired' AND target_user_id = $GT_ID")" = 1 ]

pstep "vanity hold: released after 30 days, then anyone premium can claim it"
q "UPDATE guilds SET vanity_paused_at = now() - interval '31 days' WHERE id = $GA" >/dev/null
sweep
[ -z "$(q "SELECT vanity_code FROM guilds WHERE id = $GA")" ]
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GA AND action = 'vanity_released'")" = 1 ]
expect 404 GET "/api/invites/$VC" ""
expect 200 PUT "/api/guilds/$GB/vanity" "{\"code\":\"$VC\"}" "$PL"
expect 200 GET "/api/invites/$VC" ""; ok --arg g "$GB" '.guild.id == $g'
expect 200 DELETE "/api/guilds/$GB/vanity" "" "$PL"; ok '.vanity == null'
expect 404 GET "/api/invites/$VC" ""

pstep "the Premium page: list and filters; outhouse grant revoked"
expect 200 GET "/api/safety/premium?status=all" "" "$OWNER_T"
ok --arg u "gt$RUN" --arg g "Plain Loo" 'any(.entitlements[]; .kind == "user" and .target_name == $u) and any(.entitlements[]; .kind == "outhouse" and .target_name == $g and .active)'
expect 200 GET "/api/safety/premium?kind=people&status=ended&q=gt$RUN" "" "$OWNER_T"
ok 'length > 0 and (.entitlements | length) >= 2 and all(.entitlements[]; .kind == "user" and (.active | not))'
expect 200 GET "/api/safety/premium?kind=outhouses&status=active" "" "$OWNER_T"; ok 'all(.entitlements[]; .kind == "outhouse" and .active)'
expect 200 POST "/api/safety/outhouses/$GB/premium/revoke" '{}' "$OWNER_T"; ok '.active == false'
expect 200 GET /api/guilds "" "$PL"; ok --arg g "$GB" 'any(.[]; .id == $g and (.premium | not))'
form 413 "$PL" "/api/channels/$CB/drafts" -F "file=@$P/26mb.bin"
expect 200 GET /api/safety/log "" "$OWNER_T"
ok '[.entries[].action] | (index("premium_grant") and index("premium_revoke") and index("premium_expired"))'
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GB AND action = 'premium_off'")" -ge 1 ]

pstep "the owner's switches: free-for-everyone emoji and longer messages"
expect 200 PUT /api/safety/premium-settings '{"emoji_premium":false,"message_chars":4000}' "$OWNER_T"
ok '.emoji_premium == false and .message_chars == 4000 and .premium_message_chars == 4000'
expect 200 POST "/api/channels/$CA/messages" "{\"content\":\"$LONG4000\"}" "$PL"
expect 200 POST "/api/channels/$CA/messages" '{"content":":plainpog:"}' "$PL"; ok --arg e "$TOKE" '.content == $e'
expect 400 PUT /api/safety/premium-settings '{"message_chars":50}' "$OWNER_T"
expect 200 PUT /api/safety/premium-settings '{"reset":true}' "$OWNER_T"; ok '.emoji_premium == true and .message_chars == 2000'
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'premium_settings'")" -ge 3 ]
rm -rf "$P"
echo "Premium test passed."
