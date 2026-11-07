#!/usr/bin/env bash
# Backs up Outhouse: the database and all uploaded files.
#   ./backup.sh            make a backup now
# setup.sh installs a nightly run (3:30 am). Backups go to ~/outhouse-backups
# (change with BACKUP_DIR) and the newest 14 are kept (KEEP_DAYS).
#
# Off-site copies (optional, set in deploy/.env; setup.sh can set them up):
#   OFFSITE_DIR=/mnt/outhouse-backups   a mounted folder, e.g. a Windows share
#   OFFSITE_KEEP=14                     how many copies to keep there
#   RCLONE_REMOTE=gdrive:outhouse       an rclone remote (if rclone is installed)
#   RCLONE_KEEP=14
# Each run writes status/backup.json, which the server's status page and
# healthcheck.sh read (so a failed backup or copy sends an alert).
set -euo pipefail
cd "$(dirname "$0")"

# Read KEY=VALUE lines from .env without running it, keeping anything
# already set in the environment.
load_env() {
  local file=$1 line key value
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    [[ "$line" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]] || continue
    key=${line%%=*}
    value=${line#*=}
    value=${value#\"}; value=${value%\"}
    if [ -z "${!key+x}" ]; then export "$key=$value"; fi
  done < "$file"
}
load_env .env

BACKUP_DIR="${BACKUP_DIR:-$HOME/outhouse-backups}"
KEEP_DAYS="${KEEP_DAYS:-14}"
OFFSITE_DIR="${OFFSITE_DIR:-}"
OFFSITE_KEEP="${OFFSITE_KEEP:-$KEEP_DAYS}"
RCLONE_REMOTE="${RCLONE_REMOTE:-}"
RCLONE_KEEP="${RCLONE_KEEP:-$KEEP_DAYS}"
STATUS_FILE="${BACKUP_STATUS_FILE:-./status/backup.json}"
STAMP=$(date +%Y-%m-%d_%H%M)
NAME="outhouse-$STAMP.tgz"
DOCKER="docker"
docker info >/dev/null 2>&1 || DOCKER="sudo docker"

json_string() {
  python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1" 2>/dev/null \
    || printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g' | tr -d '\n\r')"
}

# write_status OK(true|false) FILE SIZE OFFSITE MESSAGE
write_status() {
  mkdir -p "$(dirname "$STATUS_FILE")" 2>/dev/null || true
  {
    printf '{"time":%s,"ok":%s,"file":%s,"size":%s,' \
      "$(json_string "$(date -Iseconds)")" "$1" "$(json_string "$2")" "$(json_string "$3")"
    printf '"offsite":%s,"message":%s}\n' "$(json_string "$4")" "$(json_string "$5")"
  } > "$STATUS_FILE.tmp" 2>/dev/null && mv "$STATUS_FILE.tmp" "$STATUS_FILE" \
    || echo "(couldn't write $STATUS_FILE)"
}

STEP="starting"
on_error() {
  echo "Backup FAILED while $STEP."
  write_status false "" "" "off" "Failed while $STEP (see ~/outhouse-backups/backup.log)"
}
trap on_error ERR

mkdir -p "$BACKUP_DIR"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

STEP="dumping the database"
echo "Backing up the database..."
$DOCKER compose exec -T db pg_dump -U outhouse --format=custom outhouse > "$WORK/database.dump"

STEP="copying uploaded files"
echo "Backing up uploaded files..."
$DOCKER compose exec -T server tar -C /data -cf - uploads > "$WORK/uploads.tar"

STEP="packing the backup"
cp .env "$WORK/env" 2>/dev/null || true
tar -C "$WORK" -czf "$BACKUP_DIR/$NAME" .
chmod 600 "$BACKUP_DIR/$NAME"

# Keep the newest $KEEP_DAYS backups.
# shellcheck disable=SC2012 # backup names are plain (outhouse-<date>.tgz)
ls -1t "$BACKUP_DIR"/outhouse-*.tgz 2>/dev/null | tail -n +"$((KEEP_DAYS + 1))" | xargs -r rm -f
SIZE=$(du -h "$BACKUP_DIR/$NAME" | cut -f1)
echo "Saved $BACKUP_DIR/$NAME ($SIZE)"
trap - ERR

# ---- Off-site copies ----
OFFSITE_OK=()
OFFSITE_FAILED=()

copy_to_dir() {
  local dir=$1
  # Trigger an automount, then make sure it's really a mounted share: copying
  # into an empty mount point would just fill this VM's own disk.
  ls "$dir" >/dev/null 2>&1 || true
  if [ ! -d "$dir" ]; then
    OFFSITE_FAILED+=("$dir doesn't exist"); return
  fi
  if [ "${OFFSITE_REQUIRE_MOUNT:-yes}" = yes ] && command -v mountpoint >/dev/null 2>&1 \
     && ! mountpoint -q "$dir"; then
    OFFSITE_FAILED+=("$dir isn't mounted (is the Windows PC on and the share reachable?)"); return
  fi
  if ! cp "$BACKUP_DIR/$NAME" "$dir/$NAME.part" 2>"$WORK/cp.err" || ! mv "$dir/$NAME.part" "$dir/$NAME"; then
    rm -f "$dir/$NAME.part" 2>/dev/null || true
    OFFSITE_FAILED+=("copying to $dir failed: $(head -c 200 "$WORK/cp.err" | tr '\n' ' ')"); return
  fi
  # shellcheck disable=SC2012
  ls -1t "$dir"/outhouse-*.tgz 2>/dev/null | tail -n +"$((OFFSITE_KEEP + 1))" | xargs -r rm -f
  OFFSITE_OK+=("$dir")
  echo "Copied to $dir (keeping $OFFSITE_KEEP)"
}

copy_to_rclone() {
  local remote=$1
  if ! command -v rclone >/dev/null 2>&1; then
    OFFSITE_FAILED+=("rclone isn't installed (sudo apt install rclone)"); return
  fi
  if ! rclone copy "$BACKUP_DIR/$NAME" "$remote" 2>"$WORK/rclone.err"; then
    OFFSITE_FAILED+=("rclone to $remote failed: $(tail -c 200 "$WORK/rclone.err" | tr '\n' ' ')"); return
  fi
  rclone lsf "$remote" --include 'outhouse-*.tgz' 2>/dev/null | sort -r \
    | tail -n +"$((RCLONE_KEEP + 1))" | while IFS= read -r old; do
        rclone deletefile "${remote%/}/$old" || true
      done || true
  OFFSITE_OK+=("$remote")
  echo "Copied to $remote (keeping $RCLONE_KEEP)"
}

[ -n "$OFFSITE_DIR" ] && copy_to_dir "$OFFSITE_DIR"
[ -n "$RCLONE_REMOTE" ] && copy_to_rclone "$RCLONE_REMOTE"

if [ ${#OFFSITE_FAILED[@]} -gt 0 ]; then
  OFFSITE="failed: $(printf '%s; ' "${OFFSITE_FAILED[@]}")"; OFFSITE=${OFFSITE%; }
  echo "Off-site copy FAILED: ${OFFSITE#failed: }"
elif [ ${#OFFSITE_OK[@]} -gt 0 ]; then
  OFFSITE="ok: $(printf '%s, ' "${OFFSITE_OK[@]}")"; OFFSITE=${OFFSITE%, }
else
  OFFSITE="off"
fi
write_status true "$NAME" "$SIZE" "$OFFSITE" ""
[ ${#OFFSITE_FAILED[@]} -eq 0 ] || exit 2
