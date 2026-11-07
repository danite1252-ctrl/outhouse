#!/usr/bin/env bash
# Uptime alerts for Outhouse. Checks the server (and its database), voice
# (LiveKit) and the last backup, and when something changes (goes down, comes
# back, a backup fails) sends an alert to an ntfy topic and/or a webhook
# (Discord-style JSON, so a Discord or Outhouse webhook URL works).
#   ./healthcheck.sh          check now (setup.sh runs it every 5 minutes)
# Settings, in deploy/.env (all optional):
#   ALERT_NTFY_URL=https://ntfy.sh/your-secret-topic
#   ALERT_WEBHOOK_URL=https://discord.com/api/webhooks/...
#   HEALTH_URL=...           default: http://127.0.0.1:8080/api/health behind
#                            Apache, else https://$DOMAIN/api/health
#   LIVEKIT_HEALTH_URL=...   default http://127.0.0.1:7880 ("off" to skip)
#   BACKUP_MAX_AGE_HOURS=36  alert if the newest backup is older than this
# Settings already in the environment win over .env (handy for testing).
set -uo pipefail
cd "$(dirname "$0")" || exit 1

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

STATUS_DIR=${STATUS_DIR:-./status}
STATE_FILE="$STATUS_DIR/health.state"
BACKUP_STATUS_FILE=${BACKUP_STATUS_FILE:-$STATUS_DIR/backup.json}
BACKUP_MAX_AGE_HOURS=${BACKUP_MAX_AGE_HOURS:-36}
ALERT_NAME=${ALERT_NAME:-Outhouse${DOMAIN:+ ($DOMAIN)}}
if [ -z "${HEALTH_URL:-}" ]; then
  if [[ "${COMPOSE_PROFILES:-}" == *behind-proxy* || -z "${DOMAIN:-}" ]]; then
    HEALTH_URL=http://127.0.0.1:8080/api/health
  else
    HEALTH_URL="https://${DOMAIN}/api/health"
  fi
fi
LIVEKIT_HEALTH_URL=${LIVEKIT_HEALTH_URL:-http://127.0.0.1:7880}
mkdir -p "$STATUS_DIR"

# ---- checks: each sets <name>_state (a short word) and <name>_detail ----

check_server() {
  local body code
  body=$(curl -sS -m 10 -w '\n%{http_code}' "$HEALTH_URL" 2>&1)
  code=${body##*$'\n'}
  body=${body%$'\n'*}
  if [ "$code" = 200 ] && [[ "$body" == *'"db":true'* ]]; then
    server_state=up; server_detail="Outhouse is answering at $HEALTH_URL again."
  elif [ "$code" = 503 ] || { [ "$code" = 200 ] && [[ "$body" == *'"db":false'* ]]; }; then
    server_state=db_down; server_detail="Outhouse is running but its database isn't answering ($HEALTH_URL)."
  else
    local why="no answer"
    [[ "$code" =~ ^[0-9]{3}$ ]] && [ "$code" != 000 ] && why="HTTP $code"
    server_state=down; server_detail="Outhouse isn't answering at $HEALTH_URL ($why)."
  fi
}

check_livekit() {
  if [ "$LIVEKIT_HEALTH_URL" = off ]; then livekit_state=off; livekit_detail=""; return; fi
  if curl -fsS -m 5 -o /dev/null "$LIVEKIT_HEALTH_URL" 2>/dev/null; then
    livekit_state=up; livekit_detail="Voice (LiveKit) is answering again."
  else
    livekit_state=down; livekit_detail="Voice (LiveKit) isn't answering at $LIVEKIT_HEALTH_URL, so nobody can join voice."
  fi
}

check_backup() {
  backup_state=unknown; backup_detail=""
  [ -f "$BACKUP_STATUS_FILE" ] || return 0
  local out
  out=$(python3 - "$BACKUP_STATUS_FILE" "$BACKUP_MAX_AGE_HOURS" <<'PY' 2>/dev/null
import json, sys
from datetime import datetime, timezone
try:
    s = json.load(open(sys.argv[1]))
except Exception as e:
    print(f"unreadable\tCan't read the backup status file: {e}")
    sys.exit()
max_age = float(sys.argv[2])
when = s.get("time") or ""
try:
    t = datetime.fromisoformat(when.replace("Z", "+00:00"))
    if t.tzinfo is None:
        t = t.astimezone()
    age = (datetime.now(timezone.utc) - t).total_seconds() / 3600
except Exception:
    age = None
offsite = s.get("offsite") or "off"
if s.get("ok") is False:
    print(f"failed\tThe last backup ({when}) failed: {s.get('message') or 'see backup.log'}")
elif age is not None and age > max_age:
    print(f"stale\tThe newest backup is {age:.0f} hours old (from {when}). Is the nightly cron job running?")
elif offsite.startswith("failed"):
    print(f"copy_failed\tThe backup worked, but the off-site copy didn't: {offsite[len('failed'):].lstrip(': ')}")
else:
    print(f"ok\tBackups are working again (last one {when}, {s.get('size') or '?'}; off-site copy: {offsite}).")
PY
)
  [ -n "$out" ] || return 0
  backup_state=${out%%$'\t'*}
  backup_detail=${out#*$'\t'}
}

# ---- alerts ----

json_string() { python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }

send_alert() { # send_alert TITLE MESSAGE PRIORITY(default|high) TAG
  local title=$1 message=$2 priority=$3 tag=$4 sent=0
  if [ -n "${ALERT_NTFY_URL:-}" ]; then
    if curl -fsS -m 15 -o /dev/null -H "Title: $title" -H "Priority: $priority" -H "Tags: $tag" \
         --data-binary "$message" "$ALERT_NTFY_URL"; then sent=1
    else echo "$(date -Iseconds) couldn't send the alert to ntfy ($ALERT_NTFY_URL)"; fi
  fi
  if [ -n "${ALERT_WEBHOOK_URL:-}" ]; then
    local payload
    payload="{\"username\":\"Outhouse monitor\",\"content\":$(json_string "**$title**
$message")}"
    if curl -fsS -m 15 -o /dev/null -H "Content-Type: application/json" \
         --data-binary "$payload" "$ALERT_WEBHOOK_URL"; then sent=1
    else echo "$(date -Iseconds) couldn't send the alert to the webhook"; fi
  fi
  [ "$sent" = 1 ] || [ -z "${ALERT_NTFY_URL:-}${ALERT_WEBHOOK_URL:-}" ] || return 1
  return 0
}

check_server
check_livekit
check_backup

declare -A old=() new=()
if [ -f "$STATE_FILE" ]; then
  while IFS='=' read -r k v; do [ -n "$k" ] && old[$k]=$v; done < "$STATE_FILE"
fi

alert_for() { # alert_for NAME STATE DETAIL GOOD_STATES...
  local name=$1 state=$2 detail=$3; shift 3
  local before=${old[$name]:-} good=0 s ok=0
  for s in "$@"; do [ "$state" = "$s" ] && good=1; done
  new[$name]=$state
  [ "$state" = "$before" ] && return 0
  if [ -z "$before" ] && [ "$good" = 1 ]; then return 0; fi   # first run, all fine
  if [ "$state" = unknown ] || [ "$state" = off ]; then return 0; fi
  local label
  case $name in
    server) label="server" ;;
    livekit) label="voice" ;;
    backup) label="backups" ;;
  esac
  if [ "$good" = 1 ]; then
    send_alert "$ALERT_NAME: $label back up" "$detail" default white_check_mark && ok=1
  else
    case $state in
      db_down) send_alert "$ALERT_NAME: database down" "$detail" high rotating_light && ok=1 ;;
      failed) send_alert "$ALERT_NAME: backup failed" "$detail" high warning && ok=1 ;;
      copy_failed) send_alert "$ALERT_NAME: off-site backup copy failed" "$detail" default warning && ok=1 ;;
      stale) send_alert "$ALERT_NAME: backups have stopped" "$detail" high warning && ok=1 ;;
      *) send_alert "$ALERT_NAME: $label down" "$detail" high rotating_light && ok=1 ;;
    esac
  fi
  echo "$(date -Iseconds) $name: ${before:-?} -> $state"
  # Couldn't tell anyone: keep the old state so the next run tries again.
  [ "$ok" = 1 ] || new[$name]=$before
}

alert_for server "$server_state" "$server_detail" up
alert_for livekit "$livekit_state" "$livekit_detail" up off
alert_for backup "$backup_state" "$backup_detail" ok unknown

printf 'server=%s\nlivekit=%s\nbackup=%s\nchecked=%s\n' \
  "${new[server]}" "${new[livekit]}" "${new[backup]}" "$(date -Iseconds)" > "$STATE_FILE.tmp" \
  && mv "$STATE_FILE.tmp" "$STATE_FILE"
exit 0
