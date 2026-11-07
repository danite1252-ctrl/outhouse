#!/usr/bin/env bash
# Backs up Outhouse: the database and all uploaded files.
#   ./backup.sh            make a backup now
# setup.sh installs a nightly run (3:30 am). Backups go to ~/outhouse-backups
# (change with BACKUP_DIR) and the newest 14 are kept (KEEP_DAYS).
# Copy that folder somewhere else now and then (another PC, a USB drive,
# cloud storage): a backup on the same machine won't survive a dead disk.
set -euo pipefail
cd "$(dirname "$0")"

BACKUP_DIR="${BACKUP_DIR:-$HOME/outhouse-backups}"
KEEP_DAYS="${KEEP_DAYS:-14}"
STAMP=$(date +%Y-%m-%d_%H%M)
DOCKER="docker"
docker info >/dev/null 2>&1 || DOCKER="sudo docker"

mkdir -p "$BACKUP_DIR"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "Backing up the database..."
$DOCKER compose exec -T db pg_dump -U outhouse --format=custom outhouse > "$WORK/database.dump"

echo "Backing up uploaded files..."
$DOCKER compose exec -T server tar -C /data -cf - uploads > "$WORK/uploads.tar"

cp .env "$WORK/env" 2>/dev/null || true
tar -C "$WORK" -czf "$BACKUP_DIR/outhouse-$STAMP.tgz" .
chmod 600 "$BACKUP_DIR/outhouse-$STAMP.tgz"

# Keep the newest $KEEP_DAYS backups.
ls -1t "$BACKUP_DIR"/outhouse-*.tgz 2>/dev/null | tail -n +"$((KEEP_DAYS + 1))" | xargs -r rm -f

echo "Saved $BACKUP_DIR/outhouse-$STAMP.tgz ($(du -h "$BACKUP_DIR/outhouse-$STAMP.tgz" | cut -f1))"
