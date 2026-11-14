#!/usr/bin/env bash
# "My raids" across outhouses, sidebar widgets, realm time and the Mentions
# inbox, end to end against a real server and Postgres. ci/smoke.sh runs it;
# it can also run on its own against a local server started with
# OUTHOUSE_TEST_HOOKS=1:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=postgres://... bash ci/widgets_test.sh
set -euo pipefail
trap 'echo "Widgets test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
RUN=${WIDGETS_RUN:-$(date +%s | tail -c 6)}
WB=/tmp/widgets-body
wstep() { echo "== widgets: $*"; }

# req TOKEN METHOD PATH [JSON] -> HTTP status (body in $WB)
req() {
  local token=$1 method=$2 path=$3 body=${4:-}
  local args=(-sS -X "$method" -o "$WB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
# ok TOKEN METHOD PATH [JSON] -> prints the body; fails unless 2xx
ok() {
  local code; code=$(req "$@")
  if [[ "$code" != 2* ]]; then echo "FAILED: $2 $3 -> $code"; cat "$WB"; echo; return 1; fi
  cat "$WB"
}
# no WANT TOKEN METHOD PATH [JSON] -> fails unless refused with WANT
no() {
  local want=$1; shift
  local code; code=$(req "$@")
  if [ "$code" != "$want" ]; then echo "EXPECTED $want: $2 $3 -> $code"; cat "$WB"; echo; return 1; fi
}
# Each account from its own address (registration is rate limited per address).
N=0
reg() {
  N=$((N + 1))
  curl -fsS -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.41.$N" \
    -d '{"username":"'"$1$RUN"'","email":"'"$1$RUN"'@example.test","password":"password123","birth_date":"1990-01-01","display_name":"'"$2"'"}' \
    "$BASE/api/auth/register"
}
iso() { date -u -d "$1" +%Y-%m-%dT%H:%M:%SZ; }
# wait_mentions TOKEN JQ_FILTER [JQ ARGS...] -> waits for the inbox (recorded in the background) to match
wait_mentions() {
  local token=$1 filter=$2; shift 2
  for _ in $(seq 1 40); do
    ok "$token" GET "/api/users/@me/mentions?limit=50" | jq -e "$@" "$filter" >/dev/null 2>&1 && return 0
    sleep 0.25
  done
  echo "the inbox never matched $filter:"
  ok "$token" GET "/api/users/@me/mentions?limit=50" | jq -c '{unread, m: [.mentions[] | {c: .message.content, kind, unread}]}'
  return 1
}

R1=$(reg wraid "Wren"); T1=$(jq -r .token <<<"$R1"); U1=$(jq -r .user.id <<<"$R1")
R2=$(reg wbard "Bard"); T2=$(jq -r .token <<<"$R2"); U2=$(jq -r .user.id <<<"$R2")
R3=$(reg wcrow "Crow"); T3=$(jq -r .token <<<"$R3"); U3=$(jq -r .user.id <<<"$R3")
R4=$(reg wdove "Dove"); T4=$(jq -r .token <<<"$R4"); U4=$(jq -r .user.id <<<"$R4")

# Two outhouses: Bard is in both, Crow and Dove only in the first.
G1=$(ok "$T1" POST /api/guilds '{"name":"Widget Raiders"}' | jq -r .id)
G2=$(ok "$T1" POST /api/guilds '{"name":"Widget Family"}' | jq -r .id)
RAIDS=$(ok "$T1" POST "/api/guilds/$G1/channels" '{"name":"raids"}' | jq -r .id)
OFFICERS=$(ok "$T1" POST "/api/guilds/$G1/channels" '{"name":"officers"}' | jq -r .id)
ok "$T1" PUT "/api/channels/$OFFICERS/permissions/$G1" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
FCHAN=$(ok "$T1" POST "/api/guilds/$G2/channels" '{"name":"family"}' | jq -r .id)
INV1=$(ok "$T1" POST "/api/guilds/$G1/invites" | jq -r .code)
INV2=$(ok "$T1" POST "/api/guilds/$G2/invites" | jq -r .code)
for t in "$T2" "$T3" "$T4"; do ok "$t" POST "/api/invites/$INV1" >/dev/null; done
ok "$T2" POST "/api/invites/$INV2" >/dev/null

# ---------------------------------------------------------------------------
wstep "my raids: every outhouse at once, only channels you can see, your own status"
no 401 "" GET "/api/users/@me/raids?from=$(iso now)&to=$(iso '+1 day')"
S1=$(iso '+3 hours'); S2=$(iso '+1 day'); S3=$(iso '+2 days'); S4=$(iso '+3 days'); OLD=$(iso '-2 days')
E1=$(ok "$T1" POST "/api/channels/$RAIDS/events" '{"title":"Molten Core","starts_at":"'"$S1"'","duration_minutes":180}' | jq -r .event.id)
E2=$(ok "$T1" POST "/api/channels/$RAIDS/events" '{"title":"Onyxia","starts_at":"'"$S2"'"}' | jq -r .event.id)
E3=$(ok "$T1" POST "/api/channels/$OFFICERS/events" '{"title":"Officer raid","starts_at":"'"$S3"'"}' | jq -r .event.id)
E4=$(ok "$T1" POST "/api/channels/$FCHAN/events" '{"title":"Family Kara","starts_at":"'"$S4"'"}' | jq -r .event.id)
E5=$(ok "$T1" POST "/api/channels/$RAIDS/events" '{"title":"Last week","starts_at":"'"$S4"'"}' | jq -r .event.id)
# (Raids can't be planned in the past: move this one there.)
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = '$OLD' WHERE id = $E5"
ok "$T2" PUT "/api/events/$E1/signup" '{"status":"tentative","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
ok "$T2" PUT "/api/events/$E2/signup" '{"status":"no","role":""}' >/dev/null
ok "$T2" PUT "/api/events/$E4/signup" '{"status":"bench","role":"dps","class":"Mage","spec":"Frost"}' >/dev/null
FROM=$(iso '-7 days'); TO=$(iso '+20 days')
ok "$T2" GET "/api/users/@me/raids?from=$FROM&to=$TO" > /tmp/myraids.json
jq -e --arg e1 "$E1" --arg e2 "$E2" --arg e4 "$E4" --arg e5 "$E5" --arg g1 "$G1" --arg g2 "$G2" '
  [.[].id] == [$e5, $e1, $e2, $e4]
  and (.[] | select(.id == $e4) | .guild_id == $g2)
  and (.[] | select(.id == $e1) | .guild_id == $g1)' /tmp/myraids.json >/dev/null
# Bard's own statuses come with each raid (tentative / can't make it / bench / nothing yet).
jq -e --arg u "$U2" --arg e1 "$E1" --arg e2 "$E2" --arg e4 "$E4" --arg e5 "$E5" '
  def mine($id): [.[] | select(.id == $id) | .signups[] | select(.user_id == $u) | .status];
  mine($e1) == ["tentative"] and mine($e2) == ["no"] and mine($e4) == ["bench"] and mine($e5) == []' /tmp/myraids.json >/dev/null
# The officers' raid is hidden from members; its owner sees it.
jq -e --arg e3 "$E3" 'all(.[]; .id != $e3)' /tmp/myraids.json >/dev/null
ok "$T1" GET "/api/users/@me/raids?from=$FROM&to=$TO" | jq -e --arg e3 "$E3" 'any(.[]; .id == $e3) and length == 5' >/dev/null
# Crow isn't in the family outhouse.
ok "$T3" GET "/api/users/@me/raids?from=$FROM&to=$TO" | jq -e --arg e4 "$E4" 'length == 3 and all(.[]; .id != $e4)' >/dev/null
# One outhouse only (the mini calendar), and the range.
ok "$T2" GET "/api/users/@me/raids?from=$FROM&to=$TO&guild=$G2" | jq -e --arg e4 "$E4" '[.[].id] == [$e4]' >/dev/null
ok "$T2" GET "/api/users/@me/raids?from=$(iso now)&to=$(iso '+30 hours')" | jq -e --arg e1 "$E1" --arg e2 "$E2" '[.[].id] == [$e1, $e2]' >/dev/null
ok "$T3" GET "/api/users/@me/raids?from=$FROM&to=$TO&guild=$G2" | jq -e 'length == 0' >/dev/null
no 400 "$T2" GET "/api/users/@me/raids?from=$FROM&to=$(iso '+90 days')"
no 400 "$T2" GET "/api/users/@me/raids?from=$TO&to=$FROM"
no 400 "$T2" GET "/api/users/@me/raids?from=$FROM&to=$TO&guild=abc"
# Lose access to a channel: its raids go.
ok "$T1" PUT "/api/channels/$RAIDS/permissions/$U3" '{"kind":"member","allow":"0","deny":"1"}' >/dev/null
ok "$T3" GET "/api/users/@me/raids?from=$FROM&to=$TO" | jq -e 'length == 0' >/dev/null
ok "$T1" DELETE "/api/channels/$RAIDS/permissions/$U3" >/dev/null

# ---------------------------------------------------------------------------
wstep "widgets: defaults, saved per person, validated, synced"
no 401 "" GET /api/users/@me/widgets
ok "$T2" GET /api/users/@me/widgets | jq -e '[.widgets[] | [.id, .on]] == [["clock",true],["calendar",true],["mentions",true],["soundboard",false]]
  and (.widgets[1].mode == "week") and all(.widgets[]; .collapsed == false)' >/dev/null
rm -f /tmp/gw-widgets.jsonl; node ci/gateway_probe.mjs "$BASE" "$T2" /tmp/gw-widgets.jsonl 20 & GWW=$!
for _ in $(seq 1 40); do grep -q '"READY"' /tmp/gw-widgets.jsonl 2>/dev/null && break; sleep 0.25; done
ok "$T2" PUT /api/users/@me/widgets '{"widgets":[{"id":"soundboard","on":true},{"id":"mentions","on":true,"collapsed":true},{"id":"calendar","on":false,"mode":"month"}]}' \
  | jq -e '[.widgets[] | .id] == ["soundboard","mentions","calendar","clock"] and .widgets[0].on and .widgets[1].collapsed
      and (.widgets[2].on | not) and .widgets[2].mode == "month" and .widgets[3].on' >/dev/null
for _ in $(seq 1 40); do grep -q '"WIDGETS_CHANGED"' /tmp/gw-widgets.jsonl 2>/dev/null && break; sleep 0.25; done
grep -q '"WIDGETS_CHANGED"' /tmp/gw-widgets.jsonl
kill $GWW 2>/dev/null || true
ok "$T2" GET /api/users/@me/widgets | jq -e '[.widgets[] | .id] == ["soundboard","mentions","calendar","clock"]' >/dev/null
# Someone else's are their own.
ok "$T3" GET /api/users/@me/widgets | jq -e '.widgets[0].id == "clock"' >/dev/null
no 400 "$T2" PUT /api/users/@me/widgets '{"widgets":[{"id":"weather","on":true}]}'
no 400 "$T2" PUT /api/users/@me/widgets '{"widgets":[{"id":"clock","on":true},{"id":"clock","on":false}]}'
no 400 "$T2" PUT /api/users/@me/widgets '{"widgets":[{"id":"calendar","on":true,"mode":"year"}]}'
no 400 "$T2" PUT /api/users/@me/widgets '{"widgets":[{"id":"clock","on":true,"mode":"month"}]}'
ok "$T2" GET /api/users/@me/widgets | jq -e '.widgets[0].id == "soundboard"' >/dev/null

# ---------------------------------------------------------------------------
wstep "realm time: an IANA zone and a region, validated, Manage Outhouse only"
ok "$T2" GET "/api/guilds/$G1/raid-settings" | jq -e '.realm_time_zone == "" and .realm_region == ""' >/dev/null
ok "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":"Australia/Sydney"}' \
  | jq -e '.realm_time_zone == "Australia/Sydney" and .realm_effective_time_zone == "Australia/Sydney" and (.realm_time_zone_derived | not)' >/dev/null
# A region on its own: its usual zone (and the region picks the weekly reset).
ok "$T1" PUT "/api/guilds/$G2/raid-settings" '{"realm_region":"EU"}' \
  | jq -e '.realm_region == "eu" and .realm_effective_region == "eu" and .realm_effective_time_zone == "Europe/Paris" and .realm_time_zone_derived' >/dev/null
# Other settings leave them alone.
ok "$T1" PUT "/api/guilds/$G1/raid-settings" '{"card_cleanup_single":"after"}' | jq -e '.realm_time_zone == "Australia/Sydney"' >/dev/null
no 400 "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":"Mars/Olympus_Mons"}'
no 400 "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":"'"$(printf 'x%.0s' $(seq 1 80))"'"}'
no 400 "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_region":"oce"}'
no 403 "$T2" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":"Europe/Paris"}'
ok "$T2" GET "/api/guilds/$G1/raid-settings" | jq -e '.realm_effective_time_zone == "Australia/Sydney"' >/dev/null
# Back to automatic.
ok "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":""}' | jq -e '.realm_time_zone == ""' >/dev/null
ok "$T1" GET "/api/guilds/$G1/audit-log" | jq -e '[.. | strings | select(test("Realm time zone: automatic → Australia/Sydney"))] | length >= 1' >/dev/null
ok "$T1" PUT "/api/guilds/$G1/raid-settings" '{"realm_time_zone":"America/New_York"}' >/dev/null

# ---------------------------------------------------------------------------
wstep "mentions: direct, role, @everyone and replies are recorded (DMs too)"
no 401 "" GET /api/users/@me/mentions
ROLE=$(ok "$T1" POST "/api/guilds/$G1/roles" '{"name":"Healers"}' | jq -r .id)
ok "$T1" PUT "/api/guilds/$G1/members/$U3/roles/$ROLE" >/dev/null
rm -f /tmp/gw-mentions.jsonl; node ci/gateway_probe.mjs "$BASE" "$T3" /tmp/gw-mentions.jsonl 30 & GWM=$!
for _ in $(seq 1 40); do grep -q '"READY"' /tmp/gw-mentions.jsonl 2>/dev/null && break; sleep 0.25; done
# (Crow's own message first: posting marks the channel read up to it.)
MC=$(ok "$T3" POST "/api/channels/$RAIDS/messages" '{"content":"can I bring an alt?"}' | jq -r .id)
M1=$(ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"<@'"$U3"'> bring your healer"}' | jq -r .id)
M2=$(ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"<@&'"$ROLE"'> check the comp"}' | jq -r .id)
M3=$(ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"@everyone pull at 8"}' | jq -r .id)
ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"no ping here"}' >/dev/null
M4=$(ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"sure","reply_to_id":"'"$MC"'"}' | jq -r .id)
# The officers channel: Crow can't see it, so nothing for Crow.
ok "$T1" POST "/api/channels/$OFFICERS/messages" '{"content":"<@'"$U3"'> is on thin ice"}' >/dev/null
wait_mentions "$T3" '
  [.mentions[] | [.message.id, .kind, .unread]] == [[$d,"reply",true],[$c,"everyone",true],[$b,"role",true],[$a,"user",true]]
  and .unread == 4 and (.has_more | not) and all(.mentions[]; .channel_name == "raids")' --arg a "$M1" --arg b "$M2" --arg c "$M3" --arg d "$M4"
ok "$T3" GET /api/users/@me/mentions/count | jq -e '.unread == 4' >/dev/null
grep -q '"MENTIONS_CHANGED"' /tmp/gw-mentions.jsonl
kill $GWM 2>/dev/null || true
# The author never gets their own; Bard got the @everyone only.
wait_mentions "$T2" '[.mentions[] | .message.id] == [$c]' --arg c "$M3"
ok "$T1" GET /api/users/@me/mentions | jq -e '.mentions == [] and .unread == 0' >/dev/null
# DMs: a direct ping in a DM is a mention, a plain DM isn't.
DM=$(ok "$T2" POST /api/dms '{"user_ids":["'"$U3"'"]}' | jq -r .id)
ok "$T2" POST "/api/channels/$DM/messages" '{"content":"hi"}' >/dev/null
MD=$(ok "$T2" POST "/api/channels/$DM/messages" '{"content":"<@'"$U3"'> you there?"}' | jq -r .id)
wait_mentions "$T3" '.mentions[0].message.id == $m and .mentions[0].guild_id == "0" and .unread == 5' --arg m "$MD"
ok "$T3" GET "/api/users/@me/mentions?guild=0" | jq -e --arg m "$MD" '[.mentions[] | .message.id] == [$m]' >/dev/null
ok "$T3" GET "/api/users/@me/mentions?guild=$G1" | jq -e '(.mentions | length) > 0 and all(.mentions[]; .guild_id != "0")' >/dev/null

wstep "mentions: not for blocked authors or hidden channels; edits, deletes and safety hides follow"
# Dove blocks Wren: Wren's pings don't reach Dove's inbox (and older ones hide).
ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"<@'"$U4"'> before the block"}' >/dev/null
wait_mentions "$T4" '.unread == 2'
ok "$T4" PUT "/api/users/@me/blocks/$U1" >/dev/null
ok "$T4" GET /api/users/@me/mentions | jq -e '.mentions == [] and .unread == 0' >/dev/null
ok "$T1" POST "/api/channels/$RAIDS/messages" '{"content":"<@'"$U4"'> after the block"}' >/dev/null
sleep 1
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM mentions WHERE user_id = $U4")" = 2 ]
ok "$T4" DELETE "/api/users/@me/blocks/$U1" >/dev/null
ok "$T4" GET /api/users/@me/mentions | jq -e '.unread == 2' >/dev/null
# Nothing for the hidden channel.
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM mentions WHERE user_id = $U3 AND channel_id = $OFFICERS")" = 0 ]
# Losing access later hides them too.
ok "$T1" PUT "/api/channels/$RAIDS/permissions/$U3" '{"kind":"member","allow":"0","deny":"1"}' >/dev/null
ok "$T3" GET /api/users/@me/mentions | jq -e --arg m "$MD" '[.mentions[] | .message.id] == [$m] and .unread == 1' >/dev/null
ok "$T1" DELETE "/api/channels/$RAIDS/permissions/$U3" >/dev/null
# Editing a ping out removes it; editing one in adds it.
ME=$(ok "$T1" POST "/api/channels/$FCHAN/messages" '{"content":"<@'"$U2"'> dinner?"}' | jq -r .id)
wait_mentions "$T2" 'any(.mentions[]; .message.id == $m)' --arg m "$ME"
ok "$T1" PATCH "/api/channels/$FCHAN/messages/$ME" '{"content":"dinner?"}' >/dev/null
wait_mentions "$T2" 'all(.mentions[]; .message.id != $m)' --arg m "$ME"
ok "$T1" PATCH "/api/channels/$FCHAN/messages/$ME" '{"content":"dinner <@'"$U2"'>?"}' >/dev/null
wait_mentions "$T2" 'any(.mentions[]; .message.id == $m and .kind == "user")' --arg m "$ME"
# A deleted message takes its mention with it.
ok "$T1" DELETE "/api/channels/$FCHAN/messages/$ME" >/dev/null
ok "$T2" GET /api/users/@me/mentions | jq -e --arg m "$ME" 'all(.mentions[]; .message.id != $m)' >/dev/null
# Hidden by a safety review: gone from the inbox.
psql "$DATABASE_URL" -qc "UPDATE messages SET safety_hidden = true WHERE id = $M1"
ok "$T3" GET /api/users/@me/mentions | jq -e --arg m "$M1" 'all(.mentions[]; .message.id != $m) and .unread == 4' >/dev/null
psql "$DATABASE_URL" -qc "UPDATE messages SET safety_hidden = false WHERE id = $M1"

wstep "mentions: read and unread, pages, filters"
# Reading the channel up to the role ping reads the older ones too.
ok "$T3" POST "/api/channels/$RAIDS/ack" '{"message_id":"'"$M2"'"}' >/dev/null
ok "$T3" GET /api/users/@me/mentions | jq -e --arg a "$M1" --arg b "$M2" --arg c "$M3" '
  .unread == 3 and ([.mentions[] | select(.unread) | .message.id] | index($a) == null and index($b) == null and index($c) != null)' >/dev/null
ok "$T3" GET "/api/users/@me/mentions?filter=unread" | jq -e '(.mentions | length) == 3 and all(.mentions[]; .unread)' >/dev/null
ok "$T3" POST /api/users/@me/mentions/read '{"message_ids":["'"$M3"'"]}' | jq -e '.marked == 1 and .unread == 2' >/dev/null
ok "$T3" POST /api/users/@me/mentions/read '{"all":true,"guild_id":"0"}' | jq -e '.unread == 1' >/dev/null
ok "$T3" POST /api/users/@me/mentions/read '{"all":true}' | jq -e '.unread == 0' >/dev/null
ok "$T3" GET "/api/users/@me/mentions?filter=unread" | jq -e '.mentions == []' >/dev/null
no 400 "$T3" POST /api/users/@me/mentions/read '{}'
no 400 "$T3" GET "/api/users/@me/mentions?filter=loud"
# Someone else's IDs do nothing.
ok "$T2" POST /api/users/@me/mentions/read '{"message_ids":["'"$M1"'"]}' | jq -e '.marked == 0' >/dev/null
# Pages: newest first, `before` for the next page.
for i in $(seq 1 7); do ok "$T1" POST "/api/channels/$FCHAN/messages" '{"content":"<@'"$U2"'> page '"$i"'"}' >/dev/null; done
wait_mentions "$T2" '[.mentions[] | select(.message.content | test("page"))] | length == 7'
P1=$(ok "$T2" GET "/api/users/@me/mentions?limit=3")
jq -e '(.mentions | length) == 3 and .has_more and .mentions[0].message.content == ("<@" + "'"$U2"'" + "> page 7")' <<<"$P1" >/dev/null
LAST=$(jq -r '.mentions[-1].message.id' <<<"$P1")
P2=$(ok "$T2" GET "/api/users/@me/mentions?limit=3&before=$LAST")
jq -e --arg l "$LAST" '(.mentions | length) == 3 and all(.mentions[]; (.message.id | tonumber) < ($l | tonumber))
  and .mentions[0].message.content == ("<@" + "'"$U2"'" + "> page 4")' <<<"$P2" >/dev/null
ok "$T2" GET "/api/users/@me/mentions?limit=50&guild=$G2" | jq -e '(.mentions | length) == 7 and (.has_more | not)' >/dev/null

wstep "mentions: older than 30 days aren't shown; older than 90 days are removed"
psql "$DATABASE_URL" -qc "UPDATE mentions SET created_at = now() - interval '40 days' WHERE user_id = $U2 AND message_id = $M3"
ok "$T2" GET "/api/users/@me/mentions?limit=50" | jq -e --arg m "$M3" 'all(.mentions[]; .message.id != $m)' >/dev/null
psql "$DATABASE_URL" -qc "UPDATE mentions SET created_at = now() - interval '100 days' WHERE user_id = $U2 AND message_id = $M3"
ok "$T2" POST /api/users/@me/mentions/cleanup | jq -e '.removed >= 1' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM mentions WHERE user_id = $U2 AND message_id = $M3")" = 0 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM mentions WHERE user_id = $U3 AND message_id = $M3")" = 1 ]

echo "Widgets test passed."
