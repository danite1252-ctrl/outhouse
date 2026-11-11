#!/usr/bin/env bash
# Packages a report's original files for police over SSH, without anyone
# opening them. The usual way is in the app: the safety console's "Hand over
# to police" makes a one-time download link for police (the same package).
# This is the fallback for when the app can't be used.
#   ./evidence-handover.sh <report-id> [output-dir]       (default: ~/outhouse-evidence)
#   ./evidence-handover.sh --any <report-id> [output-dir]  (a report that isn't child safety
#                                                          and has no quarantined files)
#   ./evidence-handover.sh --ref "Det. Smith, 1234/26" <report-id> ...
#                                                         (officer / police reference)
#   ./evidence-handover.sh --zip <report-id> ...           (a .zip as well as the .tar.gz,
#                                                          if zip is installed)
# Run it on the server (over SSH) from the deploy folder or anywhere: it finds
# docker compose next to itself. It makes
#   evidence-<id>-<UTC time>.tar.gz   (readable only by you) containing
#     evidence-<id>-<UTC time>/
#       files/<storage key>    each file, byte for byte, never opened or decoded
#       report-<id>.json       the evidence package (everything Outhouse knows)
#       HANDOVER.txt           what this is and how to check it
#       MANIFEST.sha256        SHA-256 of everything above
# The server builds it (the same code as the app's police link) and writes
# "Evidence packaged for police" (with the package's SHA-256) to the safety
# log. It never prints what's in the files.
#
# DO NOT OPEN THE FILES. Give the package to police as they tell you (they copy
# it here under supervision, or you upload it to their portal link).
#
# For tests: OUTHOUSE_CMD runs the server's commands another way (e.g.
# OUTHOUSE_CMD=./target/debug/outhouse-server with DATABASE_URL and UPLOAD_DIR set).
set -euo pipefail
umask 077

ANY=()
ZIP=no
REF=""
while [ $# -gt 0 ]; do
  case "$1" in
    --any) ANY=(--any); shift ;;
    --zip) ZIP=yes; shift ;;
    --ref|--reference) REF=${2:-}; shift 2 ;;
    -h|--help) sed -n '2,31p' "$0"; exit 0 ;;
    --) shift; break ;;
    -*) echo "Unknown option $1 (see --help)"; exit 2 ;;
    *) break ;;
  esac
done
REPORT_ID=${1:-}
if ! [[ "$REPORT_ID" =~ ^[0-9]+$ ]]; then
  echo "Usage: $0 [--any] [--zip] [--ref TEXT] <report-id> [output-dir]"
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

mkdir -p "$OUT_BASE"
TMP=$(mktemp "$OUT_BASE/.evidence-$REPORT_ID-XXXXXX")
ZIPDIR=""
trap 'rm -f "$TMP"; [ -z "$ZIPDIR" ] || rm -rf "$ZIPDIR"' EXIT

echo "Packing report $REPORT_ID on the server..."
ARGS=(evidence-package "$REPORT_ID" "${ANY[@]}" --operator "$INVOKER on $(hostname)")
[ -z "$REF" ] || ARGS+=(--reference "$REF")
set +e
oh "${ARGS[@]}" > "$TMP"
status=$?
set -e
case $status in
  0) ;;
  2) exit 2 ;;
  3) exit 3 ;;   # not child safety, no quarantined files (the server said why)
  4) exit 4 ;;   # no such report
  *) echo "Couldn't pack report $REPORT_ID (is Outhouse running? docker compose ps)"; exit 1 ;;
esac
[ -s "$TMP" ] || { echo "The package came back empty."; exit 1; }
# The name inside (evidence-<id>-<UTC time>) names the file.
NAME=$(tar -tzf "$TMP" | awk -F/ 'NR == 1 { print $1 }')
[[ "$NAME" =~ ^evidence-[0-9]+-[0-9TZ]+$ ]] || { echo "The package doesn't look right."; exit 1; }
PKG="$OUT_BASE/$NAME.tar.gz"
mv "$TMP" "$PKG"
PKGS=("$PKG")
if [ "$ZIP" = yes ]; then
  if command -v zip >/dev/null 2>&1; then
    ZIPDIR=$(mktemp -d)
    tar -C "$ZIPDIR" -xzf "$PKG"
    ( cd "$ZIPDIR" && zip -qr "$OUT_BASE/$NAME.zip" "$NAME" )
    PKGS+=("$OUT_BASE/$NAME.zip")
  else
    echo "(zip isn't installed: sudo apt install zip. The .tar.gz is ready anyway.)"
  fi
fi
for f in "${PKGS[@]}"; do chmod 600 "$f"; done
if [ "$(id -u)" = 0 ] && [ -n "${SUDO_USER:-}" ]; then
  chown "$SUDO_USER:" "${PKGS[@]}" 2>/dev/null || true
fi

echo
echo "Done. Report $REPORT_ID, packed by the server (see the lines above):"
for f in "${PKGS[@]}"; do
  echo "  $f"
  echo "  SHA-256 $(sha256sum "$f" | cut -d' ' -f1)"
done
echo "The .tar.gz's SHA-256 is in the safety log too. Police check it inside with:"
echo "  tar -xzf $NAME.tar.gz && cd $NAME && sha256sum -c MANIFEST.sha256"
echo "Do not open these files. Give the package to police as they tell you."
