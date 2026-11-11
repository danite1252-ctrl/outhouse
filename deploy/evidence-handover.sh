#!/usr/bin/env bash
# Packages a report's original files for police, without anyone opening them.
#   ./evidence-handover.sh <report-id> [output-dir]       (default: ~/outhouse-evidence)
#   ./evidence-handover.sh --any <report-id> [output-dir]  (a report that isn't child safety
#                                                          and has no quarantined files)
#   ./evidence-handover.sh --zip <report-id> ...           (a .zip as well as the .tar.gz,
#                                                          if zip is installed)
# Run it on the server (over SSH) from the deploy folder or anywhere: it finds
# docker compose next to itself. It makes
#   evidence-<id>-<UTC time>/
#     files/<storage key>      each file, byte for byte, never opened or decoded
#     report-<id>.json         the evidence package (as the console's export,
#                              plus attempts from blocked networks)
#     HANDOVER.txt             what this is and how to check it
#     MANIFEST.sha256          SHA-256 of everything above
#   evidence-<id>-<UTC time>.tar.gz   all of it in one file (readable only by you)
# and writes "Evidence packaged for police" (with the package's SHA-256) to
# the safety log. It never prints what's in the files.
#
# DO NOT OPEN THE FILES. Give the package to police as they tell you (they copy
# it here under supervision, or you upload it to their portal link).
#
# For tests: OUTHOUSE_CMD runs the server's commands another way (e.g.
# OUTHOUSE_CMD=./target/debug/outhouse-server with DATABASE_URL and UPLOAD_DIR set).
set -euo pipefail
umask 077

ANY=""
ZIP=no
while [ $# -gt 0 ]; do
  case "$1" in
    --any) ANY="--any"; shift ;;
    --zip) ZIP=yes; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    --) shift; break ;;
    -*) echo "Unknown option $1 (see --help)"; exit 2 ;;
    *) break ;;
  esac
done
REPORT_ID=${1:-}
if ! [[ "$REPORT_ID" =~ ^[0-9]+$ ]]; then
  echo "Usage: $0 [--any] [--zip] <report-id> [output-dir]"
  echo "The report id is on the report in the safety console (Hand over to police)."
  exit 2
fi
INVOKER=${SUDO_USER:-${USER:-$(id -un)}}
INVOKER_HOME=$(getent passwd "$INVOKER" 2>/dev/null | cut -d: -f6 || true)
OUT_BASE=${2:-${INVOKER_HOME:-$HOME}/outhouse-evidence}

if [ -n "${OUTHOUSE_CMD:-}" ]; then
  # shellcheck disable=SC2206 # a command and its arguments, split on purpose
  CMD=($OUTHOUSE_CMD)
else
  cd "$(dirname "$0")"
  DOCKER="docker"
  docker info >/dev/null 2>&1 || DOCKER="sudo docker"
  # shellcheck disable=SC2206
  CMD=($DOCKER compose exec -T server /app/outhouse-server)
fi
oh() { "${CMD[@]}" "$@"; }

echo "Checking report $REPORT_ID..."
LIST=$(mktemp)
trap 'rm -f "$LIST"' EXIT
set +e
oh evidence-files "$REPORT_ID" $ANY > "$LIST"
status=$?
set -e
case $status in
  0) ;;
  3) exit 3 ;;   # not child safety, no quarantined files (the server said why)
  4) exit 4 ;;   # no such report
  *) echo "Couldn't read report $REPORT_ID (is Outhouse running? docker compose ps)"; exit 1 ;;
esac

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
NAME="evidence-$REPORT_ID-$STAMP"
mkdir -p "$OUT_BASE"
DIR="$OUT_BASE/$NAME"
mkdir -p "$DIR/files"
chmod 700 "$DIR" "$DIR/files"

COUNT=0
MISSING=()
MISMATCH=()
while IFS=$'\t' read -r KEY SHA SIZE ON_DISK QUARANTINED; do
  [ -n "$KEY" ] || continue
  [[ "$KEY" =~ ^[A-Za-z0-9]+$ ]] || continue
  if [ "$ON_DISK" != yes ]; then
    MISSING+=("$KEY"); continue
  fi
  # Byte for byte, straight to disk: nothing reads, decodes or shows it.
  if ! oh evidence-cat "$REPORT_ID" "$KEY" > "$DIR/files/$KEY"; then
    rm -f "$DIR/files/$KEY"; MISSING+=("$KEY"); continue
  fi
  GOT=$(sha256sum "$DIR/files/$KEY" | cut -d' ' -f1)
  if [ "$SHA" != "-" ] && [ "$GOT" != "$SHA" ]; then MISMATCH+=("$KEY"); fi
  COUNT=$((COUNT + 1))
  echo "  copied file $COUNT ($SIZE bytes$([ "$QUARANTINED" = yes ] && echo ", quarantined"))"
done < "$LIST"

echo "Writing the evidence package..."
oh export-evidence "$REPORT_ID" --operator "$INVOKER" > "$DIR/report-$REPORT_ID.json"
[ -s "$DIR/report-$REPORT_ID.json" ] || { echo "The evidence package came back empty."; exit 1; }
if command -v python3 >/dev/null 2>&1 \
   && ! python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$DIR/report-$REPORT_ID.json"; then
  echo "The evidence package isn't valid JSON."; exit 1
fi

NOW_UTC=$(date -u '+%Y-%m-%d %H:%M:%S UTC')
NOW_SYD=$(TZ=Australia/Sydney date '+%Y-%m-%d %H:%M:%S %Z (UTC%:z), Australia/Sydney')
{
  echo "Outhouse evidence hand-over"
  echo "==========================="
  echo "Report:        $REPORT_ID"
  echo "Generated:     $NOW_UTC"
  echo "               $NOW_SYD"
  echo "Operator:      $INVOKER on $(hostname)"
  echo "Files:         $COUNT (in files/, named by their storage key; see report-$REPORT_ID.json"
  echo "               for each file's original name, type, size and the SHA-256 recorded"
  echo "               when it was reported)"
  if [ ${#MISSING[@]} -gt 0 ]; then
    echo "Not included:  ${#MISSING[@]} file(s) no longer on the server: ${MISSING[*]}"
  fi
  if [ ${#MISMATCH[@]} -gt 0 ]; then
    echo "WARNING:       ${#MISMATCH[@]} file(s) don't match the SHA-256 recorded when reported: ${MISMATCH[*]}"
  fi
  echo
  echo "The files were copied byte for byte from the Outhouse server's upload"
  echo "store. They were not opened, viewed, decoded or converted."
  echo
  echo "To check nothing has changed, in this folder run:"
  echo "    sha256sum -c MANIFEST.sha256"
  echo
  echo "Do not open these files."
} > "$DIR/HANDOVER.txt"

( cd "$DIR" && find files -type f -print0 | sort -z | xargs -0 -r sha256sum \
    && sha256sum "report-$REPORT_ID.json" HANDOVER.txt ) > "$DIR/MANIFEST.sha256"

echo "Packing..."
PKG="$OUT_BASE/$NAME.tar.gz"
tar -C "$OUT_BASE" -czf "$PKG" "$NAME"
PKGS=("$PKG")
if [ "$ZIP" = yes ]; then
  if command -v zip >/dev/null 2>&1; then
    ( cd "$OUT_BASE" && zip -qr "$NAME.zip" "$NAME" )
    PKGS+=("$OUT_BASE/$NAME.zip")
  else
    echo "(zip isn't installed: sudo apt install zip. The .tar.gz is ready anyway.)"
  fi
fi
for f in "${PKGS[@]}"; do chmod 600 "$f"; done
chmod -R go-rwx "$DIR"
if [ "$(id -u)" = 0 ] && [ -n "${SUDO_USER:-}" ]; then
  chown -R "$SUDO_USER:" "$DIR" "${PKGS[@]}" 2>/dev/null || true
fi
HASH=$(sha256sum "$PKG" | cut -d' ' -f1)

oh evidence-handover-log "$REPORT_ID" "$HASH" "$COUNT" "$NAME.tar.gz" --operator "$INVOKER" \
  || echo "(couldn't write the safety log entry; note the SHA-256 below yourself)"

echo
echo "Done. $COUNT file(s) and the evidence package for report $REPORT_ID:"
for f in "${PKGS[@]}"; do
  echo "  $f"
  echo "  SHA-256 $(sha256sum "$f" | cut -d' ' -f1)"
done
echo "  (the unpacked copy is in $DIR)"
[ ${#MISSING[@]} -eq 0 ] || echo "Note: ${#MISSING[@]} file(s) were no longer on the server (listed in HANDOVER.txt)."
[ ${#MISMATCH[@]} -eq 0 ] || echo "WARNING: ${#MISMATCH[@]} file(s) don't match their recorded SHA-256 (see HANDOVER.txt)."
echo "Do not open these files. Give the package to police as they tell you."
