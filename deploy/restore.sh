#!/usr/bin/env bash
# Restores an Outhouse backup made by backup.sh. This REPLACES everything:
# all messages, accounts and uploads since the backup are lost.
#   ./restore.sh ~/outhouse-backups/outhouse-2026-11-08_0330.tgz
set -euo pipefail
cd "$(dirname "$0")"

FILE="${1:-}"
if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
  echo "Usage: ./restore.sh <backup .tgz>"; ls -1t "${BACKUP_DIR:-$HOME/outhouse-backups}"/outhouse-*.tgz 2>/dev/null | head -5
  exit 1
fi
DOCKER="docker"
docker info >/dev/null 2>&1 || DOCKER="sudo docker"

read -rp "This replaces ALL current Outhouse data with $FILE. Type RESTORE to continue: " ok
[ "$ok" = "RESTORE" ] || { echo "Cancelled."; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
tar -C "$WORK" -xzf "$FILE"

echo "Stopping Outhouse..."
$DOCKER compose stop server

echo "Restoring the database..."
$DOCKER compose exec -T db dropdb -U outhouse --if-exists outhouse
$DOCKER compose exec -T db createdb -U outhouse outhouse
$DOCKER compose exec -T db pg_restore -U outhouse -d outhouse --no-owner < "$WORK/database.dump"

echo "Restoring uploaded files..."
$DOCKER compose run --rm --no-deps -T --entrypoint sh server -c "rm -rf /data/uploads/* && tar -C /data -xf -" < "$WORK/uploads.tar"

$DOCKER compose start server
echo "Restored. Outhouse is back up."
