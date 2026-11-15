#!/usr/bin/env bash
# Migration 0047 (one raid attendance): old hand-taken attendance rows
# (event_attendance, from before the merge) are kept, tidied and counted
# with MRT imports through the raid_attendance_outcomes view, once per
# member per raid. Builds a scratch database at migration 0046, puts in old
# rows the way the old "Take attendance" left them, applies 0047 and checks
# what counts. ci/smoke.sh runs it; on its own:
#
#   DATABASE_URL=postgres://outhouse:outhouse@localhost:5432/outhouse bash ci/attendance_migration_test.sh
set -euo pipefail
trap 'echo "Attendance migration test failed at line $LINENO"' ERR

HERE=$(cd "$(dirname "$0")" && pwd)
MIG="$HERE/../crates/server/migrations"
SERVER=${DATABASE_URL%/*}
DB="ohmig_$(date +%s)_$$"
URL="$SERVER/$DB"
psql "$SERVER/postgres" -qc "CREATE DATABASE $DB"
cleanup() { psql "$SERVER/postgres" -qc "DROP DATABASE IF EXISTS $DB" >/dev/null 2>&1 || true; }
trap cleanup EXIT
q() { psql -v ON_ERROR_STOP=1 -qtA "$URL" -c "$1"; }

echo "== attendance migration: the schema up to 0046"
for f in "$MIG"/*.sql; do
  n=$(basename "$f" | cut -c1-4)
  [ "$((10#$n))" -le 46 ] || continue
  psql -v ON_ERROR_STOP=1 -q "$URL" -f "$f" >/dev/null
done

echo "== attendance migration: old rows, as the old Take attendance left them"
q "INSERT INTO users (id, username, display_name, password_hash) VALUES
     (1, 'mig_a', 'Ana', 'x'), (2, 'mig_b', 'Bo', 'x'), (3, 'mig_c', 'Cy', 'x'),
     (4, 'mig_d', 'Di', 'x'), (5, 'mig_e', 'Ed', 'x');
   INSERT INTO guilds (id, name, owner_id) VALUES (10, 'Migrants', 1);
   INSERT INTO guild_members (guild_id, user_id) SELECT 10, id FROM users;
   INSERT INTO channels (id, guild_id, name) VALUES (20, 10, 'raids');
   INSERT INTO events (id, guild_id, channel_id, creator_id, title, starts_at, attendance_taken) VALUES
     (100, 10, 20, 1, 'Old MC (marked by hand)', now() - interval '20 days', false),
     (101, 10, 20, 1, 'BT (MRT and a hand mark)', now() - interval '10 days', true),
     (102, 10, 20, 1, 'ZG (no attendance)', now() - interval '5 days', true);
   INSERT INTO event_signups (event_id, user_id, status) VALUES
     (100, 1, 'yes'), (100, 2, 'yes'), (100, 3, 'tentative'), (100, 4, 'no'),
     (101, 1, 'yes'), (101, 2, 'yes'), (101, 3, 'yes'), (101, 4, 'bench'),
     (102, 1, 'yes');
   INSERT INTO event_attendance (event_id, user_id, status, marked_by) VALUES
     (100, 1, 'present', 1), (100, 2, ' Absent', 1), (100, 4, 'late', 1), (100, 5, 'benched', 1),
     (100, 3, 'asleep', 1),
     (101, 2, 'present', 1);
   INSERT INTO attendance_imports (id, guild_id, event_id, imported_by, boss_count, player_count) VALUES (500, 10, 101, 1, 1, 2);
   INSERT INTO attendance_pulls (id, guild_id, event_id, import_id, boss_name, boss_key, pulled_local, pulled_at)
     VALUES (600, 10, 101, 500, 'Supremus', 'bt:supremus', now() - interval '10 days', now() - interval '10 days');
   INSERT INTO attendance_marks (pull_id, event_id, character_name, character_key, user_id, present) VALUES
     (600, 101, 'Anachar', 'anachar', 1, true), (600, 101, 'Bochar', 'bochar', 2, false),
     (600, 101, 'Stranger', 'stranger', NULL, true)"

echo "== attendance migration: applying 0047"
psql -v ON_ERROR_STOP=1 -q "$URL" -f "$MIG/0047_attendance_merge.sql" >/dev/null

echo "== attendance migration: old rows kept (spelling tidied, the impossible one dropped)"
[ "$(q "SELECT count(*) FROM event_attendance")" = 5 ]
[ "$(q "SELECT status FROM event_attendance WHERE event_id = 100 AND user_id = 2")" = absent ]
[ "$(q "SELECT count(*) FROM event_attendance WHERE event_id = 100 AND user_id = 3")" = 0 ]
if q "INSERT INTO event_attendance (event_id, user_id, status) VALUES (102, 2, 'asleep')" 2>/dev/null; then
  echo "a mark that isn't one got in"; exit 1
fi
q "INSERT INTO event_attendance (event_id, user_id, status) VALUES (101, 3, 'excused')"
# "Attendance taken" follows the hand marks.
[ "$(q "SELECT string_agg(id || ':' || attendance_taken, ' ' ORDER BY id) FROM events")" = "100:true 101:true 102:false" ]

echo "== attendance migration: what counts (one row per member per raid)"
[ "$(q "SELECT count(*) FROM (SELECT 1 FROM raid_attendance_outcomes GROUP BY event_id, user_id HAVING count(*) > 1) d")" = 0 ]
GOT=$(q "SELECT string_agg(event_id || '/' || user_id || '=' || outcome, ' ' ORDER BY event_id, user_id) FROM raid_attendance_outcomes")
# 100 (hand marks only): Ana present; Bo marked absent after signing up (no-show);
#   Cy tentative and unmarked (no-show); Di said no but came late; Ed benched without signing up.
# 101 (MRT + hand): Ana there for the boss; Bo missed by MRT, marked present (the mark wins);
#   Cy excused (not a no-show); Di on the bench (never counted).
# 102 has no attendance: nothing counts.
WANT="100/1=attended 100/2=no_show 100/3=no_show 100/4=attended 100/5=attended 101/1=attended 101/2=attended 101/3=excused"
if [ "$GOT" != "$WANT" ]; then echo "got:  $GOT"; echo "want: $WANT"; exit 1; fi

echo "== attendance migration: undoing the import keeps the hand marks"
q "DELETE FROM attendance_pulls WHERE import_id = 500"
[ "$(q "SELECT count(*) FROM event_attendance WHERE event_id = 101")" = 2 ]
GOT=$(q "SELECT string_agg(user_id || '=' || outcome, ' ' ORDER BY user_id) FROM raid_attendance_outcomes WHERE event_id = 101")
[ "$GOT" = "1=no_show 2=attended 3=excused" ] || { echo "after undo: $GOT"; exit 1; }

echo "Attendance migration test passed."
