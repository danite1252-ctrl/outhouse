#!/usr/bin/env bash
# Loot history from Gargul's awarded-loot export, end to end against a real
# server and Postgres: reading the owner's real export (ci/gargul-sample.csv),
# matching raids by date and sign-ups, matching characters without case or
# accents, de-duplicating re-imports, won soft reserves (keep / lock /
# remove, a raid leader's override), undo, single deletes, permissions, the
# history's filters, the per-character summary and the CSV export.
# ci/smoke.sh runs it; it can also run on its own against a local server:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=postgres://... bash ci/loot_import_test.sh
set -euo pipefail
trap 'echo "Loot import test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
RUN=${LOOT_RUN:-$(date +%s | tail -c 6)}
LB=/tmp/loot-import-body
SAMPLE=$(cat "$(dirname "$0")/gargul-sample.csv")
lstep() { echo "== loot import: $*"; }

# req TOKEN METHOD PATH [JSON] -> HTTP status (body in $LB)
req() {
  local token=$1 method=$2 path=$3 body=${4:-}
  local args=(-sS -X "$method" -o "$LB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
# ok TOKEN METHOD PATH [JSON] -> prints the body; fails unless 2xx
ok() {
  local code; code=$(req "$@")
  if [[ "$code" != 2* ]]; then echo "FAILED: $2 $3 -> $code $(cat "$LB")" >&2; return 1; fi
  cat "$LB"
}
# no WANT TOKEN METHOD PATH [JSON] -> fails unless refused with WANT
no() {
  local want=$1; shift
  local code; code=$(req "$@")
  if [ "$code" != "$want" ]; then echo "EXPECTED $want: $2 $3 -> $code $(cat "$LB")" >&2; return 1; fi
}
# fails (printing the JSON) unless it matches
check() { # check [JQ ARGS...] JQ_FILTER < JSON
  local json; json=$(cat)
  if ! jq -e "$@" >/dev/null <<<"$json"; then echo "NO MATCH: ${*: -1}"; jq -c . <<<"$json" | head -c 3000; echo; return 1; fi
}
N=0
reg() {
  N=$((N + 1))
  curl -fsS -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.$((47 + RUN % 200)).$N" \
    -d '{"username":"'"$1$RUN"'","email":"'"$1$RUN"'@example.test","password":"password123","birth_date":"1990-01-01","display_name":"'"$2"'"}' \
    "$BASE/api/auth/register"
}
text() { jq -Rs . <<<"$1"; } # a string as JSON
iso() { date -u -d "$1" +%Y-%m-%dT%H:%M:%SZ; }

RA=$(reg lowner "Lora"); TA=$(jq -r .token <<<"$RA")
RM=$(reg lmags "Mags"); TM=$(jq -r .token <<<"$RM"); UM=$(jq -r .user.id <<<"$RM")
RB=$(reg lsham "Sham"); TB=$(jq -r .token <<<"$RB"); UB=$(jq -r .user.id <<<"$RB")
RC=$(reg ldani "Dani"); TC=$(jq -r .token <<<"$RC"); UC=$(jq -r .user.id <<<"$RC")
RD=$(reg lkit "Kit"); TD=$(jq -r .token <<<"$RD"); UD=$(jq -r .user.id <<<"$RD")
RE=$(reg lplain "Plain"); TE=$(jq -r .token <<<"$RE")
RX=$(reg lout "Outsider"); TX=$(jq -r .token <<<"$RX")

G=$(ok "$TA" POST /api/guilds '{"name":"Loot Importers"}' | jq -r .id)
RAIDS=$(ok "$TA" POST "/api/guilds/$G/channels" '{"name":"raids"}' | jq -r .id)
INV=$(ok "$TA" POST "/api/guilds/$G/invites" | jq -r .code)
for t in "$TM" "$TB" "$TC" "$TD" "$TE"; do ok "$t" POST "/api/invites/$INV" >/dev/null; done
RL=$(ok "$TA" POST "/api/guilds/$G/roles" '{"name":"Raid leader"}' | jq -r .id)
ok "$TA" PUT "/api/guilds/$G/members/$UM/roles/$RL" >/dev/null
# Raids are played on Sydney time: 09:00 UTC is 8 pm there.
ok "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"realm_time_zone":"Australia/Sydney"}' \
  | check '.sr_won == "lock" and .realm_time_zone == "Australia/Sydney"'

# Characters: Sham's Shaman (accents and all) signs up; Dani's Priest is only on her profile.
SHAM=$(ok "$TB" POST /api/users/@me/characters '{"name":"Shamièè","class":"Shaman","level":70,"game":"tbc"}' | jq -r .id)
DANI=$(ok "$TC" POST /api/users/@me/characters '{"name":"Danité","class":"Priest","level":70,"game":"tbc"}' | jq -r .id)
KIT=$(ok "$TD" POST /api/users/@me/characters '{"name":"Kitstab","class":"Rogue","level":70,"game":"tbc"}' | jq -r .id)
[ -n "$DANI" ] && [ "$DANI" != null ]

# Two raids, a night apart (planned ahead, then moved back to the export's dates).
F1=$(iso '+5 days'); F2=$(iso '+6 days')
R1=$(ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"BT Saturday","starts_at":"'"$F1"'","game":"tbc","loot_instance":"bt","soft_reserves":2,"reserve_lock":"manual"}' | jq -r .event.id)
R2=$(ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"SSC Sunday","starts_at":"'"$F2"'","game":"tbc","loot_instance":"ssc"}' | jq -r .event.id)
ok "$TB" PUT "/api/events/$R1/signup" '{"status":"yes","role":"healer","class":"Shaman","spec":"Restoration","character_id":"'"$SHAM"'"}' >/dev/null
ok "$TD" PUT "/api/events/$R1/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat","character_id":"'"$KIT"'"}' >/dev/null
ok "$TD" PUT "/api/events/$R2/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat","character_id":"'"$KIT"'"}' >/dev/null
# Soft reserves: Sham on Shamièè's main-spec win (32234), Kit on 32347.
ok "$TB" PUT "/api/events/$R1/reserves" '{"item_id":32234}' >/dev/null
ok "$TD" PUT "/api/events/$R1/reserves" '{"item_id":32347}' >/dev/null
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = '2026-11-08T09:00:00Z' WHERE id = $R1"
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = '2026-11-09T09:00:00Z' WHERE id = $R2"

# ---------------------------------------------------------------------------
lstep "reading the owner's export: 24 rows, accents kept, disenchanted, off-spec"
ok "$TM" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$SAMPLE")}" > /tmp/loot-preview.json
check --arg r1 "$R1" --arg r2 "$R2" --arg b "$UB" --arg c "$UC" --arg d "$UD" '
  (.rows | length == 24) and (.errors == []) and .duplicates == 0 and .sr_won == "lock"
  and ([.rows[] | select(.disenchanted)] | length == 4 and all(.[]; .character == "_disenchanted" and .matched == null and (.offspec | not)))
  and ([.rows[] | select(.character == "Shamièè")] | length == 4)
  and (.rows[0] | .character == "Shamièè" and .offspec and .item_id == 32240 and .gargul_id == "12811834116478231330"
       and .date == "2026-11-08" and .time == null and .event_id == $r1 and .matched.user_id == $b and .matched.via == "signup"
       and .matched.class == "Shaman" and (.reserved | not))
  and (.rows[1] | .item_id == 32234 and (.offspec | not) and .reserved)
  and ([.rows[] | select(.date == "2026-11-08") | .event_id] | unique == [$r1])
  and ([.rows[] | select(.date == "2026-11-09") | .event_id] | unique == [$r2])
  and (.rows[] | select(.character == "Danité") | .matched.user_id == $c and .matched.via == "character" and .matched.class == "Priest")
  and (.rows[] | select(.character == "Kitstab" and .item_id == 32347) | .matched.user_id == $d and .reserved)
  and (.rows[] | select(.character == "Druixxe" and .item_id == 32261) | .matched == null)
  and ([.raids[].id] | index($r1) != null and index($r2) != null)
  and (.raids[] | select(.id == $r1) | .local_date == "2026-11-08" and .instance_name == "Black Temple")' < /tmp/loot-preview.json
# Names the server knows; the rest come from Wowhead (or show as "Item 32240").
check 'all(.rows[]; (.item_name | type == "string") and (.quality | type == "number"))' < /tmp/loot-preview.json

lstep "raid card import: the raid's date (±1 day); other days greyed but can be ticked in"
LATE="$SAMPLE
2026-11-12,Huktur,30000,0,LATE-1"
ok "$TM" POST "/api/events/$R1/loot/preview" "{\"text\":$(text "$LATE")}" > /tmp/loot-raid-preview.json
check --arg r1 "$R1" '(.rows | length == 25) and all(.rows[]; .event_id == $r1)
  and ([.rows[] | select(.in_window | not) | .gargul_id] == ["LATE-1"])
  and ([.raids[].id] == [$r1])' < /tmp/loot-raid-preview.json
# Only this raid's night (the 8th), ticked by Gargul ID.
PICKS=$(jq -c '[.rows[] | select(.date == "2026-11-08") | {gargul_id}]' /tmp/loot-raid-preview.json)
ok "$TM" POST "/api/events/$R1/loot/import" "{\"text\":$(text "$LATE"),\"picks\":$PICKS}" > /tmp/loot-import1.json
check '.imported == 14 and .duplicates == 0 and .without_raid == 0 and .raids == 1 and .won_reserves == 2 and .removed_reserves == 0 and (.import_id | type == "string")' < /tmp/loot-import1.json
IMP1=$(jq -r .import_id /tmp/loot-import1.json)
ok "$TM" GET "/api/events/$R1" | check --arg b "$UB" --arg d "$UD" '.loot_count == 14
  and (.reserves[] | select(.user_id == $b and .item_id == 32234) | .won_on == "2026-11-08")
  and (.reserves[] | select(.user_id == $d and .item_id == 32347) | .won_on == "2026-11-08")'
ok "$TB" GET "/api/events/$R1/loot" | check --arg b "$UB" 'length == 14
  and ([.[] | select(.disenchanted)] | length == 4)
  and (.[0] | .character_name == "Shamièè" and .user_id == $b and .offspec and .character_class == "Shaman" and .awarded_on == "2026-11-08" and .gargul_id == "12811834116478231330")
  and (.[1] | .reserved and (.offspec | not))
  and all(.[]; .raid_title == "BT Saturday" and .loot_instance == "bt")'

lstep "outhouse Loot page import: the rest go to the next night's raid; re-imports skip what's in"
ok "$TM" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$SAMPLE")}" | check '.duplicates == 14 and ([.rows[] | select(.duplicate)] | length == 14)'
PICKS=$(jq -c '[.rows[] | {gargul_id, event_id}]' /tmp/loot-preview.json)
ok "$TM" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAMPLE"),\"picks\":$PICKS}" > /tmp/loot-import2.json
check '.imported == 10 and .duplicates == 14 and .without_raid == 0 and .raids == 1' < /tmp/loot-import2.json
IMP2=$(jq -r .import_id /tmp/loot-import2.json)
ok "$TM" GET "/api/events/$R2/loot" | check --arg c "$UC" 'length == 10 and (.[] | select(.character_name == "Danité") | .user_id == $c and .item_id == 31091 and .character_class == "Priest")'
# The same paste again: nothing new, nothing to undo.
ok "$TM" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAMPLE"),\"picks\":$PICKS}" | check '.imported == 0 and .duplicates == 24 and .import_id == null'
# The same rows from Gargul with Windows line ends, a BOM and quotes are still the same awards.
CRLF=$(printf '\xef\xbb\xbf%s' "$(sed 's/^\(.*\)$/"\1"/; s/,/","/g' <<<"$SAMPLE" | sed 's/$/\r/')")
ok "$TM" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$CRLF")}" | check '(.rows | length == 24) and .duplicates == 24 and (.rows[0].character == "Shamièè")'

lstep "two raids on one day: rows go to the raid the character signed up to"
F3=$(iso '+7 days'); F4=$(iso '+8 days')
R3=$(ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"Kara group A","starts_at":"'"$F3"'","game":"tbc","loot_instance":"kara"}' | jq -r .event.id)
R4=$(ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"Kara group B","starts_at":"'"$F4"'","game":"tbc","loot_instance":"kara"}' | jq -r .event.id)
ok "$TD" PUT "/api/events/$R3/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat","character_id":"'"$KIT"'"}' >/dev/null
ok "$TB" PUT "/api/events/$R4/signup" '{"status":"yes","role":"healer","class":"Shaman","spec":"Restoration","character_id":"'"$SHAM"'"}' >/dev/null
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = '2026-10-20T08:00:00Z' WHERE id = $R3"
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = '2026-10-20T10:00:00Z' WHERE id = $R4"
SAME="dateTime,character,itemID,offspec,id
2026-10-20,Kitstab,28504,0,K-1
2026-10-20,SHAMIEE,28508,1,K-2
2026-10-20,_disenchanted,28511,0,K-3
2026-10-20,Zeddicus,28509,0,K-4
2026-10-02,Kitstab,28510,0,K-5"
ok "$TM" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$SAME")}" > /tmp/loot-same.json
check --arg r3 "$R3" --arg r4 "$R4" --arg b "$UB" '
  (.rows[0].event_id == $r3) and (.rows[1] | .event_id == $r4 and .matched.user_id == $b and .character == "SHAMIEE")
  and (.rows[2] | .event_id == null and (.reason | startswith("2 raids that day")))
  and (.rows[3] | .event_id == null and .matched == null)
  and (.rows[4] | .event_id == null and .reason == "No raid that day")' < /tmp/loot-same.json
# Unassigned rows: one to a raid by hand, the rest kept without a raid; the unmatched name is kept.
PICKS=$(jq -c --arg r3 "$R3" '[.rows[] | {gargul_id, event_id: (if .gargul_id == "K-3" then $r3 else .event_id end)}]' /tmp/loot-same.json)
ok "$TM" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAME"),\"picks\":$PICKS}" > /tmp/loot-import3.json
check '.imported == 5 and .without_raid == 2 and .raids == 2' < /tmp/loot-import3.json
ok "$TM" GET "/api/guilds/$G/loot?character=zeddicus" | check '.total == 1 and (.entries[0] | .user_id == null and .character_name == "Zeddicus" and .event_id == null and .channel_id == null and .raid_title == "")'
# Members see loot kept without a raid too (they can see the raids channel).
ok "$TE" GET "/api/guilds/$G/loot?event=none" | check '.total == 2'
ok "$TE" GET "/api/guilds/$G/loot?event=none&de=1" | check '.total == 2'
ok "$TE" GET "/api/guilds/$G/loot?event=$R3&de=1" | check '.total == 2 and any(.entries[]; .disenchanted)'

lstep "won soft reserves lock the item for that character in later raids of the instance"
F5=$(iso '+3 days')
R5=$(ok "$TA" POST "/api/channels/$RAIDS/events" '{"title":"BT next week","starts_at":"'"$F5"'","game":"tbc","loot_instance":"bt","soft_reserves":1,"reserve_lock":"manual"}' | jq -r .event.id)
ok "$TB" PUT "/api/events/$R5/signup" '{"status":"yes","role":"healer","class":"Shaman","spec":"Restoration","character_id":"'"$SHAM"'"}' >/dev/null
ok "$TB" GET "/api/events/$R5/won-locks" | check '. == [{"item_id":32234,"item_name":.[0].item_name,"won_on":"2026-11-08","raid_title":"BT Saturday"}]'
no 400 "$TB" PUT "/api/events/$R5/reserves" '{"item_id":32234}'; grep -q "won" "$LB"
# Something else is fine; letting themselves off isn't.
no 403 "$TB" PUT "/api/events/$R5/reserves" '{"item_id":32234,"allow_won":true}'
no 403 "$TB" GET "/api/events/$R5/won-locks?user_id=$UM"
# A raid leader overrides: their reserve shows it was won before.
ok "$TM" GET "/api/events/$R5/won-locks?user_id=$UB" | check 'length == 1'
no 400 "$TM" PUT "/api/events/$R5/reserves" '{"item_id":32234,"user_id":"'"$UB"'"}'
ok "$TM" PUT "/api/events/$R5/reserves" '{"item_id":32234,"user_id":"'"$UB"'","allow_won":true}' >/dev/null
ok "$TB" GET "/api/events/$R5" | check --arg b "$UB" '.reserves | length == 1 and .[0].user_id == $b and .[0].won_before == "2026-11-08" and .[0].won_on == null'
ok "$TA" GET "/api/guilds/$G/audit-log?action=loot_reserve_add" | check '.entries | length == 1 and (.[0].details | contains("again"))'
# "Keep it": no lock.
ok "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"sr_won":"keep"}' | check '.sr_won == "keep"'
ok "$TB" GET "/api/events/$R5/won-locks" | check '. == []'
no 400 "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"sr_won":"sometimes"}'
no 403 "$TM" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"sr_won":"lock"}'

lstep "\"remove it\": a won reserve comes off the raid; undoing the import puts it back"
ok "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"sr_won":"remove"}' | check '.sr_won == "remove"'
# (The raid is over, so its reserves are locked: a raid leader adds it for Kit.)
ok "$TM" PUT "/api/events/$R1/reserves" '{"item_id":32526,"user_id":"'"$UD"'"}' >/dev/null
MORE="dateTime,character,itemID,offspec,id
2026-11-08,Kitstab,32526,0,M-1"
ok "$TM" POST "/api/events/$R1/loot/import" "{\"text\":$(text "$MORE"),\"picks\":[{\"gargul_id\":\"M-1\"}]}" > /tmp/loot-import4.json
check '.imported == 1 and .won_reserves == 1 and .removed_reserves == 1' < /tmp/loot-import4.json
IMP4=$(jq -r .import_id /tmp/loot-import4.json)
ok "$TD" GET "/api/events/$R1" | check --arg d "$UD" '[.reserves[] | select(.user_id == $d) | .item_id] == [32347] and .loot_count == 15'
# Undo: only the importer, once, within 24 hours.
no 403 "$TA" DELETE "/api/loot/imports/$IMP4"
ok "$TM" DELETE "/api/loot/imports/$IMP4" | check '.removed == 1'
no 400 "$TM" DELETE "/api/loot/imports/$IMP4"
ok "$TD" GET "/api/events/$R1" | check --arg d "$UD" '([.reserves[] | select(.user_id == $d) | .item_id] | sort == [32347, 32526]) and .loot_count == 14'
ok "$TM" GET "/api/guilds/$G/loot/imports" | check --arg i "$IMP4" --arg i1 "$IMP1" '(.[] | select(.id == $i) | .undone_at != null and (.can_undo | not) and .remaining == 0)
  and (.[] | select(.id == $i1) | .can_undo and .row_count == 14)'
psql "$DATABASE_URL" -qc "UPDATE loot_imports SET created_at = now() - interval '25 hours' WHERE id = $IMP2"
no 400 "$TM" DELETE "/api/loot/imports/$IMP2"; grep -q "24 hours" "$LB"
ok "$TA" PUT "/api/guilds/$G/raid-settings" '{"manager_role_ids":["'"$RL"'"],"sr_won":"lock"}' >/dev/null
ok "$TA" GET "/api/guilds/$G/audit-log?action=loot_import" | check '.entries | length == 4 and any(.[]; .details | contains("BT Saturday"))'
ok "$TA" GET "/api/guilds/$G/audit-log?action=loot_import_undo" | check '.entries | length == 1'

lstep "single delete (raid managers), logged; the raid's leaders only"
DEL=$(ok "$TE" GET "/api/guilds/$G/loot?event=$R2&character=negua&limit=1" | jq -r '.entries[0].id')
DELNAME=$(jq -r '.entries[0].item_name' "$LB")
no 403 "$TE" DELETE "/api/loot/$DEL"
no 403 "$TB" DELETE "/api/loot/$DEL"
ok "$TM" DELETE "/api/loot/$DEL" >/dev/null
no 404 "$TM" DELETE "/api/loot/$DEL"
ok "$TA" GET "/api/guilds/$G/audit-log?action=loot_award_remove" | check --arg n "$DELNAME" '.entries | length == 1 and (.[0].details | contains($n) and contains("SSC Sunday"))'
# Loot without a raid: raid managers.
NORAID=$(ok "$TM" GET "/api/guilds/$G/loot?event=none&character=kitstab" | jq -r '.entries[0].id')
no 403 "$TE" DELETE "/api/loot/$NORAID"
ok "$TM" DELETE "/api/loot/$NORAID" >/dev/null

lstep "permissions: importing is for raid managers; anyone who can see the raids can look"
no 403 "$TE" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$SAMPLE")}"
no 403 "$TE" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[]}"
no 403 "$TE" POST "/api/events/$R1/loot/preview" "{\"text\":$(text "$SAMPLE")}"
no 403 "$TB" POST "/api/events/$R1/loot/import" "{\"text\":$(text "$MORE"),\"picks\":[{\"gargul_id\":\"M-1\"}]}"
no 403 "$TE" GET "/api/guilds/$G/loot/imports"
no 404 "$TX" GET "/api/guilds/$G/loot"
no 404 "$TX" GET "/api/guilds/$G/loot/summary"
no 404 "$TX" GET "/api/guilds/$G/loot/export"
no 404 "$TX" POST "/api/guilds/$G/loot/preview" "{\"text\":$(text "$SAMPLE")}"
no 400 "$TM" POST "/api/guilds/$G/loot/preview" '{"text":"this is not a gargul export"}'
no 400 "$TM" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[]}"
# A raid in a channel the importer can't use.
no 400 "$TM" POST "/api/guilds/$G/loot/import" "{\"text\":$(text "$SAMPLE"),\"picks\":[{\"gargul_id\":\"12811834116478231330\",\"event_id\":\"123\"}]}"
# Hidden raids channel: its loot is hidden too.
ok "$TA" PUT "/api/channels/$RAIDS/permissions/$G" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
ok "$TE" GET "/api/guilds/$G/loot?de=1" | check '.total == 0'
no 404 "$TE" GET "/api/events/$R1/loot"
ok "$TA" DELETE "/api/channels/$RAIDS/permissions/$G" >/dev/null

lstep "history: filters (accent-free character, dates, raid, instance, MS/OS, disenchanted), sorting"
ok "$TE" GET "/api/guilds/$G/loot?character=SHAMIEE&limit=100" | check 'all(.entries[]; .character_name | ascii_downcase | startswith("sham")) and .total == 5'
ok "$TE" GET "/api/guilds/$G/loot?character=Shamièè&spec=os" | check '.total == 4 and all(.entries[]; .offspec)'
ok "$TE" GET "/api/guilds/$G/loot?character=Shamièè&spec=ms" | check '.total == 1 and .entries[0].item_id == 32234'
ok "$TE" GET "/api/guilds/$G/loot?from=2026-11-09&to=2026-11-09" | check '.total == 9'
ok "$TE" GET "/api/guilds/$G/loot?instance=bt&de=1" | check '.total == 14'
ok "$TE" GET "/api/guilds/$G/loot?instance=bt" | check '.total == 10 and all(.entries[]; .disenchanted | not)'
ok "$TE" GET "/api/guilds/$G/loot?sort=character&dir=asc&limit=200" | check '[.entries[].character_name | ascii_downcase] as $n | $n == ($n | sort)'
ok "$TE" GET "/api/guilds/$G/loot?sort=date&dir=asc&limit=200" | check '[.entries[].awarded_on] as $d | $d == ($d | sort) and $d[0] == "2026-10-20"'
ok "$TE" GET "/api/guilds/$G/loot?limit=3" | check '(.entries | length == 3) and .has_more and .total > 3'
no 400 "$TE" GET "/api/guilds/$G/loot?from=yesterday"

lstep "per-character summary and CSV export"
ok "$TE" GET "/api/guilds/$G/loot/summary" | check --arg b "$UB" '
  (.[] | select(.character_key == "shamiee") | .ms == 1 and .os == 4 and .total == 5 and .user_id == $b and .character_class == "Shaman" and .last_on == "2026-11-09" and .reserved == 1)
  and all(.[]; .character_key != "_disenchanted")
  and (.[] | select(.character_key == "zeddicus") | .user_id == null)'
ok "$TE" GET "/api/guilds/$G/loot/export?event=$R2" > /tmp/loot-export.json
jq -j .csv /tmp/loot-export.json > /tmp/loot-export.csv
head -1 /tmp/loot-export.csv | grep -qx 'Date,Character,Member,Class,Item ID,Item,Offspec,Disenchanted,Soft reserved,Raid,Instance,Gargul ID'
[ "$(wc -l < /tmp/loot-export.csv)" = 10 ] || { cat /tmp/loot-export.csv; exit 1; }
grep -q '^2026-11-09,Danité,Dani,Priest,31091,' /tmp/loot-export.csv
grep -q ',SSC Sunday,Serpentshrine Cavern,33618872553272877036$' /tmp/loot-export.csv
ok "$TE" GET "/api/guilds/$G/loot/export?de=1&event=$R1" | check '.rows == 14'

lstep "a member's loot by character (full profile)"
ok "$TE" GET "/api/users/$UB/loot" | check --arg g "$G" '
  length == 1 and .[0].guild_id == $g and .[0].guild_name == "Loot Importers"
  and (.[0].characters[] | select(.character_key == "shamiee") | .character_name == "Shamièè" and .total == 5 and .offspec == 4
       and .this_phase == 3 and .phase_label == "Phase 3" and (.items[0].awarded_on == "2026-11-09"))'
ok "$TX" GET "/api/users/$UB/loot" | check '. == []'

echo "Loot import test passed."
