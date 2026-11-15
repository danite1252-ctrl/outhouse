#!/usr/bin/env bash
# Channel categories and channel order (feat-categories), end to end against
# a real server and Postgres: making, renaming and deleting categories (their
# channels survive, uncategorised), channels made in a category (with
# "Sync permissions"), the list order (uncategorised text, voice, then each
# category with text above voice), PATCH /channels/positions (moves,
# validation, permissions, one audit entry, the gateway event), syncing
# permissions, who sees which categories, collapsing (per person) and no
# messages in a category.
# ci/smoke.sh runs it; it can also run on its own against a local server:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=postgres://... bash ci/categories_test.sh
set -euo pipefail
trap 'echo "Categories test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
RUN=${CAT_RUN:-$(date +%s | tail -c 6)}
CB=/tmp/categories-body
cstep() { echo "== categories: $*"; }

# req TOKEN METHOD PATH [JSON] -> HTTP status (body in $CB)
req() {
  local token=$1 method=$2 path=$3 body=${4:-}
  local args=(-sS -X "$method" -o "$CB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
ok() {
  local code; code=$(req "$@")
  if [[ "$code" != 2* ]]; then echo "FAILED: $2 $3 -> $code $(cat "$CB")" >&2; return 1; fi
  cat "$CB"
}
no() {
  local want=$1; shift
  local code; code=$(req "$@")
  if [ "$code" != "$want" ]; then echo "EXPECTED $want: $2 $3 -> $code $(cat "$CB")" >&2; return 1; fi
}
check() {
  local json; json=$(cat)
  if ! jq -e "$@" >/dev/null <<<"$json"; then echo "NO MATCH: ${*: -1}"; jq -c . <<<"$json" | head -c 3000; echo; return 1; fi
}
N=0
reg() {
  N=$((N + 1))
  curl -fsS -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.$((100 + RUN % 120)).$N" \
    -d '{"username":"'"$1$RUN"'","email":"'"$1$RUN"'@example.test","password":"password123","birth_date":"1990-01-01","display_name":"'"$2"'"}' \
    "$BASE/api/auth/register"
}
# The list as "kind:name:parent-name" lines, in order.
shape() { jq -r '(map({key: .id, value: .name}) | from_entries) as $n | .[] | "\(.kind):\(.name):\(if .parent_id then $n[.parent_id] else "-" end)"'; }
names() { jq -r '[.[] | .name] | join(",")'; }

RA=$(reg cowner "Cat Owner"); TA=$(jq -r .token <<<"$RA")
RB=$(reg cmember "Cat Member"); TB=$(jq -r .token <<<"$RB")
RM=$(reg cmod "Cat Mod"); TM=$(jq -r .token <<<"$RM"); UM=$(jq -r .user.id <<<"$RM")
RX=$(reg cout "Cat Outsider"); TX=$(jq -r .token <<<"$RX")

G=$(ok "$TA" POST /api/guilds '{"name":"Category Corner"}' | jq -r .id)
INV=$(ok "$TA" POST "/api/guilds/$G/invites" | jq -r .code)
for t in "$TB" "$TM"; do ok "$t" POST "/api/invites/$INV" >/dev/null; done
G2=$(ok "$TX" POST /api/guilds '{"name":"Somewhere Else"}' | jq -r .id)
CAT_X=$(ok "$TX" POST "/api/guilds/$G2/channels" '{"name":"Not yours","kind":4}' | jq -r .id)
# Mods manage channels (not roles).
MOD=$(ok "$TA" POST "/api/guilds/$G/roles" '{"name":"Mod","permissions":"16777216"}' | jq -r .id)
ok "$TA" PUT "/api/guilds/$G/members/$UM/roles/$MOD" >/dev/null

cstep "no categories: the old list, nothing made automatically"
ok "$TA" GET "/api/guilds/$G/channels" | check 'length == 2 and all(.[]; .kind != 4 and .parent_id == null)'
PORCH=$(ok "$TA" GET "/api/guilds/$G/channels" | jq -r '.[] | select(.kind == 0) | .id')
DROP=$(ok "$TA" GET "/api/guilds/$G/channels" | jq -r '.[] | select(.kind == 2) | .id')

cstep "make categories (names, limits, no nesting, Manage Channels)"
no 400 "$TA" POST "/api/guilds/$G/channels" '{"name":"   ","kind":4}'
LONG=$(printf 'x%.0s' $(seq 1 101))
no 400 "$TA" POST "/api/guilds/$G/channels" '{"name":"'"$LONG"'","kind":4}'
no 403 "$TB" POST "/api/guilds/$G/channels" '{"name":"Nope","kind":4}'
no 404 "$TX" POST "/api/guilds/$G/channels" '{"name":"Nope","kind":4}'
RAIDS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"  Raid   nights ","kind":4}' | jq -r .id)
ok "$TA" GET "/api/guilds/$G/channels" | check --arg id "$RAIDS" 'any(.[]; .id == $id and .kind == 4 and .name == "Raid nights" and .parent_id == null)'
no 400 "$TA" POST "/api/guilds/$G/channels" '{"name":"Nested","kind":4,"parent_id":"'"$RAIDS"'"}'
SOCIAL=$(ok "$TM" POST "/api/guilds/$G/channels" '{"name":"Social","kind":4}' | jq -r .id)
# Categories hold no messages and no voice.
no 404 "$TA" GET "/api/channels/$RAIDS/messages"
no 404 "$TA" POST "/api/channels/$RAIDS/messages" '{"content":"hello header"}'
no 400 "$TA" PATCH "/api/channels/$RAIDS" '{"topic":"no topics on categories"}'

cstep "channels in a category (and not in someone else's)"
SIGNUPS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"Sign ups","kind":0,"parent_id":"'"$RAIDS"'"}' | jq -r .id)
ok "$TA" GET "/api/guilds/$G/channels" | check --arg id "$SIGNUPS" --arg p "$RAIDS" 'any(.[]; .id == $id and .name == "sign-ups" and .parent_id == $p)'
RAIDVOICE=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"Raid comms","kind":2,"parent_id":"'"$RAIDS"'"}' | jq -r .id)
STRATS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"strats","parent_id":"'"$RAIDS"'"}' | jq -r .id)
no 400 "$TA" POST "/api/guilds/$G/channels" '{"name":"sneaky","parent_id":"'"$CAT_X"'"}'
no 400 "$TA" POST "/api/guilds/$G/channels" '{"name":"sneaky","parent_id":"'"$PORCH"'"}'
CHAT=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"chat","parent_id":"'"$SOCIAL"'"}' | jq -r .id)

cstep "list order: uncategorised text, voice, then each category (text above voice)"
EXPECT=$'0:front-porch:-\n2:The Long Drop:-\n4:Raid nights:-\n0:sign-ups:Raid nights\n0:strats:Raid nights\n2:Raid comms:Raid nights\n4:Social:-\n0:chat:Social'
GOT=$(ok "$TA" GET "/api/guilds/$G/channels" | shape)
[ "$GOT" = "$EXPECT" ] || { echo "order:"; echo "$GOT"; exit 1; }
ok "$TA" GET "/api/guilds/$G/channels" | check '[.[].position] as $p | $p == ($p | sort) and ($p | unique | length) == length'

cstep "positions endpoint: who may, and what's refused"
P=/api/guilds/$G/channels/positions
no 403 "$TB" PATCH "$P" '[{"id":"'"$STRATS"'","position":0}]'
no 404 "$TX" PATCH "$P" '[{"id":"'"$STRATS"'","position":0}]'
no 400 "$TA" PATCH "$P" '[]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$CAT_X"'","position":0}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$STRATS"'","position":0},{"id":"'"$CAT_X"'","position":1}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$SOCIAL"'","parent_id":"'"$RAIDS"'"}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$STRATS"'","parent_id":"'"$CHAT"'"}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$STRATS"'","parent_id":"'"$CAT_X"'"}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$STRATS"'","position":1},{"id":"'"$STRATS"'","position":2}]'
no 422 "$TA" PATCH "$P" '[{"id":"nope","position":1}]'
# Nothing changed by any of that.
[ "$(ok "$TA" GET "/api/guilds/$G/channels" | shape)" = "$EXPECT" ]

cstep "moves: reorder in a category, between categories, categories themselves (the gateway tells everyone)"
node ci/gateway_probe.mjs "$BASE" "$TB" /tmp/gw-cat-b.jsonl 20 &
GWPID=$!
sleep 1.5
AUDIT_BEFORE=$(ok "$TA" GET "/api/guilds/$G/audit-log?action=channels_move" | jq '.entries | length')
# strats above sign-ups; Raid comms and front-porch into Social; Social above Raid nights.
ok "$TM" PATCH "$P" '[
  {"id":"'"$PORCH"'","position":4,"parent_id":"'"$SOCIAL"'"},
  {"id":"'"$DROP"'","position":5,"parent_id":"'"$SOCIAL"'"},
  {"id":"'"$SOCIAL"'","position":0},
  {"id":"'"$CHAT"'","position":1},
  {"id":"'"$RAIDS"'","position":2},
  {"id":"'"$STRATS"'","position":3},
  {"id":"'"$SIGNUPS"'","position":4},
  {"id":"'"$RAIDVOICE"'","position":7,"parent_id":"'"$SOCIAL"'"}
]' | check 'length == 8'
EXPECT2=$'4:Social:-\n0:chat:Social\n0:front-porch:Social\n2:The Long Drop:Social\n2:Raid comms:Social\n4:Raid nights:-\n0:strats:Raid nights\n0:sign-ups:Raid nights'
GOT=$(ok "$TB" GET "/api/guilds/$G/channels" | shape)
[ "$GOT" = "$EXPECT2" ] || { echo "after move:"; echo "$GOT"; exit 1; }
ok "$TA" GET "/api/guilds/$G/channels" | check '[.[].position] == [range(0; length)]'
ok "$TA" GET "/api/guilds/$G/audit-log?action=channels_move" | check --argjson n "$AUDIT_BEFORE" '(.entries | length) == $n + 1 and (.entries[0].details | contains("#front-porch: No category → SOCIAL") and contains("🔈 Raid comms: RAID NIGHTS → SOCIAL"))'
# A text channel placed among voice channels still lists above them (Discord's rule).
ok "$TA" PATCH "$P" '[{"id":"'"$CHAT"'","position":99}]' >/dev/null
ok "$TA" GET "/api/guilds/$G/channels" | shape | grep -n . | tr '\n' ' ' | grep -q "2:0:front-porch:Social 3:0:chat:Social 4:2:The Long Drop:Social"
# Back to uncategorised with parent_id null.
ok "$TA" PATCH "$P" '[{"id":"'"$DROP"'","parent_id":null}]' >/dev/null
ok "$TA" GET "/api/guilds/$G/channels" | check --arg d "$DROP" '.[0].id == $d and .[0].parent_id == null'
for _ in $(seq 1 30); do grep -q '"CHANNELS_REORDERED"' /tmp/gw-cat-b.jsonl && break; sleep 0.3; done
grep '"CHANNELS_REORDERED"' /tmp/gw-cat-b.jsonl | head -1 | jq -e --arg g "$G" '.d.guild_id == $g and (.d | keys == ["guild_id"])' >/dev/null
kill "$GWPID" 2>/dev/null || true

cstep "a moderator can't move a channel they can't manage"
ok "$TA" PUT "/api/channels/$STRATS/permissions/$MOD" '{"kind":"role","allow":"0","deny":"16777216"}' >/dev/null
no 403 "$TM" PATCH "$P" '[{"id":"'"$STRATS"'","position":0,"parent_id":null}]'
ok "$TA" DELETE "/api/channels/$STRATS/permissions/$MOD" >/dev/null

cstep "who sees which categories"
# A private category: hidden from members when everything in it is hidden too.
OFFICERS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"Officers","kind":4}' | jq -r .id)
ok "$TA" PUT "/api/channels/$OFFICERS/permissions/$G" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
OFFTALK=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"officer-talk","parent_id":"'"$OFFICERS"'","sync_permissions":true}' | jq -r .id)
ok "$TA" GET "/api/guilds/$G/channels" | check --arg o "$OFFTALK" 'any(.[]; .id == $o and .permissions_synced == true)'
ok "$TA" GET "/api/channels/$OFFTALK/permissions" | check --arg g "$G" 'length == 1 and .[0].target_id == $g and .[0].deny == "1"'
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" --arg o "$OFFTALK" 'all(.[]; .id != $c and .id != $o)'
ok "$TA" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" 'any(.[]; .id == $c)'
# An empty category shows to people who can fill it, not to members.
EMPTY=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"Empty shelf","kind":4}' | jq -r .id)
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$EMPTY" 'all(.[]; .id != $c)'
ok "$TM" GET "/api/guilds/$G/channels" | check --arg c "$EMPTY" 'any(.[]; .id == $c)'
# A channel members can see inside a hidden category brings the header with it.
NOTICES=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"notices","parent_id":"'"$OFFICERS"'"}' | jq -r .id)
ok "$TA" GET "/api/guilds/$G/channels" | check --arg n "$NOTICES" 'any(.[]; .id == $n and .permissions_synced == false)'
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" --arg n "$NOTICES" --arg o "$OFFTALK" 'any(.[]; .id == $c) and any(.[]; .id == $n and .parent_id == $c) and all(.[]; .id != $o)'
# "View outhouse as role" works it out from the same permissions.
ok "$TA" GET "/api/guilds/$G/preview-permissions?roles=" | check --arg c "$OFFICERS" --arg o "$OFFTALK" --arg n "$NOTICES" '(.channels[$c] | tonumber) % 2 == 0 and (.channels[$o] | tonumber) % 2 == 0 and (.channels[$n] | tonumber) % 2 == 1'

cstep "sync permissions with the category (a copy; nothing inherited)"
no 403 "$TB" POST "/api/channels/$NOTICES/sync-permissions"
no 403 "$TM" POST "/api/channels/$NOTICES/sync-permissions"
no 400 "$TA" POST "/api/channels/$DROP/sync-permissions"
ok "$TA" POST "/api/channels/$NOTICES/sync-permissions" | check '.permissions_synced == true'
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" --arg n "$NOTICES" 'all(.[]; .id != $c and .id != $n)'
ok "$TA" GET "/api/guilds/$G/audit-log?action=channel_permissions_sync" | check '.entries[0].target_name == "#notices" and (.entries[0].details | contains("OFFICERS"))'
# Changing the category later doesn't change its channels (no inheritance)...
ok "$TA" PUT "/api/channels/$OFFICERS/permissions/$G" '{"kind":"role","allow":"1","deny":"0"}' >/dev/null
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" --arg n "$NOTICES" 'all(.[]; .id != $c and .id != $n)'
ok "$TM" GET "/api/guilds/$G/channels" | check --arg c "$OFFICERS" --arg n "$NOTICES" 'any(.[]; .id == $c) and all(.[]; .id != $n)'
ok "$TA" GET "/api/guilds/$G/channels" | check --arg n "$NOTICES" 'any(.[]; .id == $n and .permissions_synced == false)'
# ...until it's synced again, here as part of a move.
ok "$TA" PATCH "$P" '[{"id":"'"$NOTICES"'","parent_id":"'"$OFFICERS"'","sync_permissions":true}]' >/dev/null
ok "$TB" GET "/api/guilds/$G/channels" | check --arg n "$NOTICES" --arg c "$OFFICERS" 'any(.[]; .id == $n and .permissions_synced == true) and any(.[]; .id == $c)'
no 403 "$TM" PATCH "$P" '[{"id":"'"$CHAT"'","sync_permissions":true}]'
no 400 "$TA" PATCH "$P" '[{"id":"'"$DROP"'","sync_permissions":true}]'

cstep "collapse is per person"
no 404 "$TA" PUT "/api/channels/$PORCH/collapse" '{"collapsed":true}'
no 404 "$TX" PUT "/api/channels/$RAIDS/collapse" '{"collapsed":true}'
ok "$TA" PUT "/api/channels/$RAIDS/collapse" '{"collapsed":true}' >/dev/null
ok "$TA" GET "/api/guilds/$G/channels" | check --arg c "$RAIDS" 'any(.[]; .id == $c and .collapsed == true)'
ok "$TB" GET "/api/guilds/$G/channels" | check --arg c "$RAIDS" 'any(.[]; .id == $c and .collapsed == false)'
ok "$TA" PUT "/api/channels/$RAIDS/collapse" '{"collapsed":false}' >/dev/null
ok "$TA" GET "/api/guilds/$G/channels" | check --arg c "$RAIDS" 'any(.[]; .id == $c and .collapsed == false)'

cstep "rename a category"
no 403 "$TB" PATCH "/api/channels/$RAIDS" '{"name":"Mine now"}'
no 400 "$TA" PATCH "/api/channels/$RAIDS" '{"name":"'"$LONG"'"}'
ok "$TA" PATCH "/api/channels/$RAIDS" '{"name":"Raid Nights & Alts"}' | check '.name == "Raid Nights & Alts" and .kind == 4'
ok "$TA" GET "/api/guilds/$G/audit-log?action=category_update" | check '.entries[0].details == "Name: Raid nights → Raid Nights & Alts"'

cstep "delete a category: its channels stay, uncategorised"
no 403 "$TB" DELETE "/api/channels/$RAIDS"
ok "$TA" DELETE "/api/channels/$RAIDS" >/dev/null
ok "$TA" GET "/api/guilds/$G/channels" | check --arg c "$RAIDS" --arg s "$STRATS" --arg u "$SIGNUPS" 'all(.[]; .id != $c) and any(.[]; .id == $s and .parent_id == null) and any(.[]; .id == $u and .parent_id == null)'
ok "$TA" GET "/api/guilds/$G/channels" | shape | head -4 | tr '\n' ' ' | grep -q "^0:strats:- 0:sign-ups:- 2:The Long Drop:- 4:Social:-"
ok "$TA" GET "/api/guilds/$G/channels" | check '[.[].position] == [range(0; length)]'
ok "$TA" POST "/api/channels/$STRATS/messages" '{"content":"still here"}' >/dev/null
ok "$TA" GET "/api/guilds/$G/audit-log?action=category_delete" | check '.entries[0].target_name == "📁 RAID NIGHTS & ALTS" and (.entries[0].details | contains("2 channels moved out of it"))'
ok "$TA" GET "/api/guilds/$G/audit-log?action=category_create" | check '.entries | length >= 4'
ok "$TA" GET "/api/guilds/$G/audit-log?action=channel_create" | check 'any(.entries[]; .details == "Text channel in OFFICERS")'
# Deleting a category nobody could see (no channels) is fine too.
ok "$TA" DELETE "/api/channels/$EMPTY" >/dev/null

echo "Categories test passed."
