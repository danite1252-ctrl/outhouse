#!/usr/bin/env bash
# Raid attendance from Method Raid Tools (MRT), end to end against a real
# server and Postgres: reading the owner's real export
# (ci/mrt-attendance-sample.tsv) and a three-boss, two-night grid with gaps
# (ci/mrt-attendance-two-nights.tsv), spaces instead of tabs, difficulty
# numbers (and boss names that end in digits), matching columns to raids and
# names to members, de-duplicating re-imports, attendance % and no-shows over
# ranges (the bench and "can't make it" never count), the no-show hint for
# raid leaders, profiles, undo / delete, the audit log and permissions.
# ci/smoke.sh runs it; it can also run on its own against a local server:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=postgres://... bash ci/attendance_test.sh
set -euo pipefail
trap 'echo "Attendance test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
RUN=${ATT_RUN:-$(date +%s | tail -c 6)}
AB=/tmp/attendance-body
HERE=$(dirname "$0")
SAMPLE=$(cat "$HERE/mrt-attendance-sample.tsv")
NIGHTS=$(cat "$HERE/mrt-attendance-two-nights.tsv")
astep() { echo "== attendance: $*"; }

# req TOKEN METHOD PATH [JSON] -> HTTP status (body in $AB)
req() {
  local token=$1 method=$2 path=$3 body=${4:-}
  local args=(-sS -X "$method" -o "$AB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
ok() {
  local code; code=$(req "$@")
  if [[ "$code" != 2* ]]; then echo "FAILED: $2 $3 -> $code $(cat "$AB")" >&2; return 1; fi
  cat "$AB"
}
no() {
  local want=$1; shift
  local code; code=$(req "$@")
  if [ "$code" != "$want" ]; then echo "EXPECTED $want: $2 $3 -> $code $(cat "$AB")" >&2; return 1; fi
}
check() {
  local json; json=$(cat)
  if ! jq -e "$@" >/dev/null <<<"$json"; then echo "NO MATCH: ${*: -1}"; jq -c . <<<"$json" | head -c 3000; echo; return 1; fi
}
N=0
reg() {
  N=$((N + 1))
  curl -fsS -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.$((49 + RUN % 150)).$N" \
    -d '{"username":"'"$1$RUN"'","email":"'"$1$RUN"'@example.test","password":"password123","birth_date":"1990-01-01","display_name":"'"$2"'"}' \
    "$BASE/api/auth/register"
}
text() { jq -Rs . <<<"$1"; }
iso() { date -u -d "$1" +%Y-%m-%dT%H:%M:%SZ; }
sql() { psql "$DATABASE_URL" -qtAc "$1"; }

RA=$(reg aowner "Lora"); TA=$(jq -r .token <<<"$RA")
RM=$(reg amags "Mags"); TM=$(jq -r .token <<<"$RM"); UM=$(jq -r .user.id <<<"$RM")
RB=$(reg asham "Sham"); TB=$(jq -r .token <<<"$RB"); UB=$(jq -r .user.id <<<"$RB")
RC=$(reg adani "Dani"); TC=$(jq -r .token <<<"$RC"); UC=$(jq -r .user.id <<<"$RC")
RD=$(reg akit "Kit"); TD=$(jq -r .token <<<"$RD"); UD=$(jq -r .user.id <<<"$RD")
RF=$(reg aavi "Avi"); TF=$(jq -r .token <<<"$RF"); UF=$(jq -r .user.id <<<"$RF")
RE=$(reg aplain "Plain"); TE=$(jq -r .token <<<"$RE"); UE=$(jq -r .user.id <<<"$RE")
RT=$(reg atent "Tent"); TT=$(jq -r .token <<<"$RT"); UT=$(jq -r .user.id <<<"$RT")
RN=$(reg abench "Benchy"); TN=$(jq -r .token <<<"$RN"); UN=$(jq -r .user.id <<<"$RN")
RS=$(reg aabs "Abs"); TS=$(jq -r .token <<<"$RS"); US=$(jq -r .user.id <<<"$RS")
RX=$(reg aout "Outsider"); TX=$(jq -r .token <<<"$RX")

G=$(ok "$TA" POST /api/guilds '{"name":"Attendance Takers"}' | jq -r .id)
RAIDS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"raids"}' | jq -r .id)
INV=$(ok "$TA" POST "/api/guilds/$G/invites" | jq -r .code)
for t in "$TM" "$TB" "$TC" "$TD" "$TF" "$TE" "$TT" "$TN" "$TS"; do ok "$t" POST "/api/invites/$INV" >/dev/null; done
RL=$(ok "$TA" POST "/api/guilds/$G/roles" '{"name":"Raid leader"}' | jq -r .id)
ok "$TA" PUT "/api/guilds/$G/members/$UM/roles/$RL" >/dev/null
ok "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"realm_time_zone":"Australia/Sydney"}' >/dev/null

SHAM=$(ok "$TB" POST /api/users/@me/characters '{"name":"Shambulàncè","class":"Shaman","level":70,"game":"tbc"}' | jq -r .id)
ok "$TC" POST /api/users/@me/characters '{"name":"Danité","class":"Priest","level":70,"game":"tbc"}' >/dev/null
KIT=$(ok "$TD" POST /api/users/@me/characters '{"name":"Kitstab","class":"Rogue","level":70,"game":"tbc"}' | jq -r .id)
AVI=$(ok "$TF" POST /api/users/@me/characters '{"name":"Avizandum","class":"Mage","level":70,"game":"tbc"}' | jq -r .id)

# Three Black Temple nights (planned ahead, then moved back to the exports' nights: 8 pm Sydney).
F=$(iso '+5 days')
mk() { ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"'"$1"'","starts_at":"'"$F"'","game":"tbc","loot_instance":"bt","max_dps":20}' | jq -r .event.id; }
R1=$(mk "BT Sunday"); R2=$(mk "BT Wednesday"); R3=$(mk "BT Thursday")
su() { ok "$2" PUT "/api/events/$1/signup" '{"status":"'"$3"'","role":"dps","class":"'"$4"'","spec":"x"'"${5:+,\"character_id\":\"$5\"}"'}' >/dev/null; }
for r in "$R1" "$R2" "$R3"; do
  su "$r" "$TB" yes Shaman "$SHAM"; su "$r" "$TD" yes Rogue "$KIT"; su "$r" "$TE" yes Warrior
done
su "$R1" "$TF" yes Mage "$AVI"; su "$R2" "$TF" late Mage "$AVI"; su "$R3" "$TF" yes Mage "$AVI"
su "$R2" "$TT" tentative Hunter
su "$R2" "$TS" no ""
ok "$TA" PUT "/api/events/$R2/signup" '{"status":"bench","role":"dps","class":"Warlock","spec":"x","user_id":"'"$UN"'"}' >/dev/null
sql "UPDATE events SET starts_at = '2026-11-15T09:00:00Z' WHERE id = $R1;
     UPDATE events SET starts_at = '2026-11-11T09:00:00Z' WHERE id = $R2;
     UPDATE events SET starts_at = '2026-11-12T09:00:00Z' WHERE id = $R3"

# ---------------------------------------------------------------------------
astep "reading the owner's export: 22 characters, 1 boss, accents kept, difficulty 4, Naj'entus matched"
ok "$TM" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$SAMPLE")}" > /tmp/att-preview.json
check --arg r1 "$R1" --arg b "$UB" --arg c "$UC" '
  (.columns | length == 1) and (.players | length == 22) and .warnings == [] and .duplicates == 0
  and (.columns[0] | .boss_name == "High Warlord Naj'"'"'entus" and .encounter_name == "High Warlord Naj'"'"'entus"
       and .encounter_key == "bt:highwarlordnajentus" and .instance_name == "Black Temple"
       and .difficulty == 4 and .difficulty_label == "25-player" and .date == "2026-11-15" and .time == "20:44"
       and .event_id == $r1 and .in_window and (.duplicate | not) and .present == 22)
  and (.players[] | select(.name == "Shambulàncè") | .key == "shambulance" and .present == [true]
       and .matches[$r1].user_id == $b and .matches[$r1].via == "signup" and .matches[$r1].class == "Shaman")
  and (.players[] | select(.name == "Danité") | .matches[$r1].user_id == $c and .matches[$r1].via == "character")
  and (.players[] | select(.name == "Rumbet") | .matches == {} and .member == null)
  and (.raids | length == 1) and (.raids[0] | .id == $r1 and .local_date == "2026-11-15" and .local_time == "20:00"
       and (.signups | length == 4) and (.signups[] | select(.user_id == $b) | .character_key == "shambulance" and .status == "yes"))' < /tmp/att-preview.json
# The outhouse's window: the column finds the raid by date and time.
ok "$TM" POST "/api/guilds/$G/attendance/mrt/preview" "{\"text\":$(text "$SAMPLE")}" | check --arg r1 "$R1" '.columns[0].event_id == $r1 and .columns[0].reason == ""'

astep "spaces instead of tabs, CRLF and a BOM read the same"
SPACED=$(printf '\xef\xbb\xbf%s' "$(sed 's/\t/    /g; s/$/\r/' <<<"$SAMPLE")")
ok "$TM" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$SPACED")}" | check '
  (.players | length == 22) and all(.players[]; .present == [true]) and .columns[0].difficulty == 4
  and any(.players[]; .name == "Shambulàncè")'

astep "header without a difficulty number; boss names that end in digits stay whole"
ODD=$(printf '\t15/11/2026 21:30:00 - Supremus\t15/11/2026 22:00:00 - Kologarn 3000\t15/11/2026 22:30:00 - Trial Dummy9\nAvizandum\tx\tx\tx\n')
ok "$TM" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$ODD")}" | check '
  (.columns[0] | .boss_name == "Supremus" and .difficulty == null and .encounter_name == "Supremus")
  and (.columns[1] | .boss_name == "Kologarn 3000" and .difficulty == null and .encounter_name == "")
  and (.columns[2] | .boss_name == "Trial Dummy9" and .difficulty == null)'
# A digit another column confirmed is the difficulty, even on a boss that isn't listed.
CONF=$(printf '\t15/11/2026 21:30:00 - Supremus4\t15/11/2026 22:00:00 - Mystery Boss4\nAvizandum\tx\tx\n')
ok "$TM" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$CONF")}" | check '.columns[1] | .boss_name == "Mystery Boss" and .difficulty == 4'
no 400 "$TM" POST "/api/events/$R1/attendance/mrt/preview" '{"text":"not an export\nat all"}'
grep -q "MRT" "$AB"

astep "importing on the raid card; the same paste again adds nothing"
ok "$TM" POST "/api/events/$R1/attendance/mrt/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"index\":0}]}" > /tmp/att-import1.json
check '.bosses == 1 and .players == 22 and .duplicates == 0 and .raids == 1 and (.import_id | type == "string")' < /tmp/att-import1.json
IMP1=$(jq -r .import_id /tmp/att-import1.json)
ok "$TE" GET "/api/events/$R1" | check '.attendance_bosses == 1'
ok "$TM" POST "/api/events/$R1/attendance/mrt/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"index\":0}]}" | check '.bosses == 0 and .duplicates == 1 and .import_id == null'
# A second leader's export of the same pull, a few seconds out, is the same pull.
SHIFTED=${SAMPLE/20:44:00/20:44:41}
ok "$TM" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$SHIFTED")}" | check '.duplicates == 1 and .columns[0].duplicate'
ok "$TM" POST "/api/events/$R1/attendance/mrt/import" "{\"text\":$(text "$SHIFTED"),\"picks\":[{\"index\":0}]}" | check '.bosses == 0 and .duplicates == 1'
ok "$TE" GET "/api/events/$R1/attendance/mrt" | check --arg b "$UB" --arg c "$UC" '
  (.pulls | length == 1) and (.pulls[0] | .boss_name == "High Warlord Naj'"'"'entus" and .difficulty == 4
     and .encounter_name == "High Warlord Naj'"'"'entus" and .pulled_at == "2026-11-15T09:44:00Z")
  and (.players | length == 22)
  and (.players[] | select(.character_key == "shambulance") | .user_id == $b and .character_name == "Shambulàncè" and .character_class == "Shaman" and .present == [true])
  and (.players[] | select(.character_key == "danite") | .user_id == $c and .character_class == "Priest")
  and (.players[] | select(.character_key == "rumbet") | .user_id == null)'

astep "three bosses over two nights, with gaps: columns go to each night's raid"
ok "$TM" POST "/api/guilds/$G/attendance/mrt/preview" "{\"text\":$(text "$NIGHTS")}" > /tmp/att-nights.json
check --arg r2 "$R2" --arg r3 "$R3" --arg d "$UD" '
  [.columns[].event_id] == [$r2, $r2, $r3]
  and [.columns[].boss_name] == ["Supremus", "Shade of Akama", "Teron Gorefiend"]
  and all(.columns[]; .difficulty == 4)
  and [.columns[].present] == [4, 4, 3]
  and (.players[] | select(.key == "kitstab") | .present == [true, false, false] and .matches[$r2].user_id == $d)
  and (.players[] | select(.key == "avizandum") | .present == [false, true, true])
  and (.players[] | select(.name == "Danité-Arugal") | .key == "danite")
  and (.players[] | select(.key == "zeddicus") | .matches == {})' < /tmp/att-nights.json
PICKS=$(jq -c '[.columns[] | {index, event_id}]' /tmp/att-nights.json)
ok "$TM" POST "/api/guilds/$G/attendance/mrt/import" "{\"text\":$(text "$NIGHTS"),\"picks\":$PICKS}" > /tmp/att-import2.json
check '.bosses == 3 and .players == 5 and .raids == 2 and .duplicates == 0' < /tmp/att-import2.json
IMP2=$(jq -r .import_id /tmp/att-import2.json)
ok "$TE" GET "/api/events/$R2/attendance/mrt" | check '[.pulls[].boss_name] == ["Supremus", "Shade of Akama"]
  and (.players[] | select(.character_key == "kitstab") | .present == [true, false])
  and (.players[] | select(.character_key == "avizandum") | .present == [false, true])'
ok "$TE" GET "/api/events/$R3/attendance/mrt" | check '(.pulls | length == 1) and (.players[] | select(.character_key == "kitstab") | .present == [false])'
# A raid in a channel the importer can't use.
no 400 "$TM" POST "/api/guilds/$G/attendance/mrt/import" "{\"text\":$(text "$NIGHTS"),\"picks\":[{\"index\":0,\"event_id\":\"123\"}]}"
no 400 "$TM" POST "/api/guilds/$G/attendance/mrt/import" "{\"text\":$(text "$NIGHTS"),\"picks\":[]}"

astep "attendance %, no-shows and last raid per character over ranges"
# Sunday 40 days ago (outside 4 weeks), Wednesday and Thursday last week.
sql "UPDATE events SET starts_at = now() - interval '40 days' WHERE id = $R1;
     UPDATE events SET starts_at = now() - interval '10 days' WHERE id = $R2;
     UPDATE events SET starts_at = now() - interval '9 days' WHERE id = $R3"
D2=$(sql "SELECT ((now() - interval '10 days') AT TIME ZONE 'Australia/Sydney')::date")
D3=$(sql "SELECT ((now() - interval '9 days') AT TIME ZONE 'Australia/Sydney')::date")
D1=$(sql "SELECT ((now() - interval '40 days') AT TIME ZONE 'Australia/Sydney')::date")
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=8w" > /tmp/att-8w.json
check --arg b "$UB" --arg d "$UD" --arg e "$UE" --arg t "$UT" --arg n "$UN" --arg s "$US" --arg d3 "$D3" --arg d2 "$D2" '
  .range == "8w" and .label == "last 8 weeks" and .raids == 3
  and (.characters[] | select(.character_key == "shambulance") | .user_id == $b and .attended == 3 and .eligible == 3 and .percent == 100 and .no_shows == 0 and .last_attended == $d3)
  and (.characters[] | select(.character_key == "kitstab") | .user_id == $d and .attended == 2 and .eligible == 3 and .percent == 67 and .no_shows == 1 and .last_attended == $d2)
  and (.characters[] | select(.character_key == "avizandum") | .attended == 3 and .percent == 100)
  and (.characters[] | select(.character_key == "danite") | .attended == 2 and .eligible == 2 and .no_shows == 0)
  and (.characters[] | select(.character_key == "zeddicus") | .user_id == null and .attended == 2 and .percent == 100)
  and (.characters[] | select(.user_id == $e) | .character_key == "plain" and .attended == 0 and .eligible == 3 and .percent == 0 and .no_shows == 3 and .last_attended == null)
  and (.characters[] | select(.user_id == $t) | .no_shows == 1 and .eligible == 1)
  and ([.characters[] | select(.user_id == $n or .user_id == $s)] == [])
  and (.characters | length == 25)' < /tmp/att-8w.json
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=4w" | check --arg e "$UE" '
  .label == "last 4 weeks" and .raids == 2
  and (.characters[] | select(.character_key == "kitstab") | .attended == 1 and .eligible == 2 and .percent == 50 and .no_shows == 1)
  and (.characters[] | select(.user_id == $e) | .eligible == 2 and .no_shows == 2)
  and ([.characters[] | select(.character_key == "rumbet")] == [])'
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=phase" | check '.range == "phase" and .label == "Phase 3" and .raids == 3'
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=custom&from=$D1&to=$D1" | check --arg d1 "$D1" '
  .raids == 1 and .from == $d1 and (.characters | length == 23)
  and (.characters[] | select(.character_key == "kitstab") | .attended == 1 and .percent == 100)'
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=all" | check '.raids == 3'
no 400 "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=custom&from=yesterday"

astep "a member's attendance by character (full profile)"
ok "$TE" GET "/api/users/$UD/attendance" | check --arg g "$G" '
  length == 1 and .[0].guild_id == $g and .[0].guild_name == "Attendance Takers"
  and (.[0].characters | length == 1)
  and (.[0].characters[0] | .character_name == "Kitstab" and .character_class == "Rogue"
       and .recent.attended == 2 and .recent.eligible == 3 and .all_time.attended == 2 and .all_time.no_shows == 1)'
ok "$TX" GET "/api/users/$UD/attendance" | check '. == []'

astep "the no-show hint: raid leaders only, 2+ no-shows in the 4 weeks before"
R4=$(mk "BT next week")
su "$R4" "$TE" yes Warrior; su "$R4" "$TD" yes Rogue "$KIT"; su "$R4" "$TB" yes Shaman "$SHAM"
ok "$TM" GET "/api/events/$R4/attendance/reliability" | check --arg e "$UE" '.days == 28 and .people == [{"user_id": $e, "no_shows": 2, "signed_up": 2}]'
ok "$TA" GET "/api/events/$R4/attendance/reliability" | check '.people | length == 1'
no 403 "$TE" GET "/api/events/$R4/attendance/reliability"
no 403 "$TD" GET "/api/events/$R4/attendance/reliability"
no 404 "$TX" GET "/api/events/$R4/attendance/reliability"
# Their own raid card stays the same for everyone: no hint in the raid itself.
ok "$TE" GET "/api/events/$R4" | check 'has("reliability") | not'

astep "permissions: importing is for raid leaders and managers; anyone who can see the raid can look"
no 403 "$TE" POST "/api/guilds/$G/attendance/mrt/preview" "{\"text\":$(text "$SAMPLE")}"
no 403 "$TE" POST "/api/guilds/$G/attendance/mrt/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"index\":0}]}"
no 403 "$TE" POST "/api/events/$R1/attendance/mrt/preview" "{\"text\":$(text "$SAMPLE")}"
no 403 "$TB" POST "/api/events/$R1/attendance/mrt/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"index\":0}]}"
no 403 "$TE" GET "/api/guilds/$G/attendance/mrt/imports"
no 404 "$TX" GET "/api/events/$R1/attendance/mrt"
no 404 "$TX" GET "/api/guilds/$G/attendance/mrt/characters"
no 404 "$TX" POST "/api/guilds/$G/attendance/mrt/preview" "{\"text\":$(text "$SAMPLE")}"
# Hidden raids channel: its attendance is hidden too.
ok "$TA" PUT "/api/channels/$RAIDS/permissions/$G" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
ok "$TE" GET "/api/guilds/$G/attendance/mrt/characters?range=all" | check '.raids == 0 and .characters == []'
no 404 "$TE" GET "/api/events/$R1/attendance/mrt"
ok "$TA" DELETE "/api/channels/$RAIDS/permissions/$G" >/dev/null

astep "undo (the importer, 24 hours) and delete (raid managers), logged"
ok "$TM" GET "/api/guilds/$G/attendance/mrt/imports" | check --arg i1 "$IMP1" --arg i2 "$IMP2" --arg r1 "$R1" '
  length == 2 and (.[] | select(.id == $i1) | .can_undo and .can_delete and .boss_count == 1 and .player_count == 22 and .remaining == 1 and .event_id == $r1 and .raids == ["BT Sunday"])
  and (.[] | select(.id == $i2) | .boss_count == 3 and .player_count == 5 and (.raids | sort) == ["BT Thursday", "BT Wednesday"] and .event_id == null)'
# Only the importer undoes; the owner (Manage Events) may delete.
no 403 "$TE" DELETE "/api/attendance/imports/$IMP1"
ok "$TM" DELETE "/api/attendance/imports/$IMP1" | check '.removed == 1'
no 400 "$TM" DELETE "/api/attendance/imports/$IMP1"
ok "$TE" GET "/api/events/$R1" | check '.attendance_bosses == 0'
ok "$TE" GET "/api/events/$R1/attendance/mrt" | check '.pulls == [] and .players == []'
# Undone: it can come in again.
ok "$TM" POST "/api/events/$R1/attendance/mrt/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"index\":0}]}" | check '.bosses == 1'
sql "UPDATE attendance_imports SET created_at = now() - interval '25 hours' WHERE id = $IMP2"
ok "$TM" GET "/api/guilds/$G/attendance/mrt/imports" | check --arg i2 "$IMP2" '.[] | select(.id == $i2) | (.can_undo | not) and .can_delete'
ok "$TA" DELETE "/api/attendance/imports/$IMP2" | check '.removed == 3'
ok "$TE" GET "/api/events/$R2" | check '.attendance_bosses == 0'
ok "$TA" GET "/api/guilds/$G/audit-log?action=attendance_import" | check '.entries | length == 3 and any(.[]; .details | contains("BT Sunday"))'
ok "$TA" GET "/api/guilds/$G/audit-log?action=attendance_import_undo" | check '.entries | length == 1'
ok "$TA" GET "/api/guilds/$G/audit-log?action=attendance_import_delete" | check '.entries | length == 1 and (.[0].details | contains("3 bosses"))'

echo "Attendance test passed."
