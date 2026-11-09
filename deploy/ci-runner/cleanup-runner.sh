#!/usr/bin/env bash
# Weekly disk clean-up for the Outhouse CI runner (installed by
# setup-linux-runner.sh as a systemd timer, outhouse-ci-cleanup.timer).
# Run by hand too:  sudo /opt/outhouse-ci/cleanup-runner.sh
# Waits while a job is running, then removes:
#   - Docker images/containers/build cache unused for a week (LiveKit, ZAP and
#     Postgres are pulled again when needed)
#   - cargo's downloaded crates older than 30 days and the runner's _work/_temp
#     (target dirs are wiped by actions/checkout every job and come back
#     from Swatinem/rust-cache)
#   - Gradle caches and old Playwright browsers older than 30 days
set -u
RUNNER_USER=${RUNNER_USER:-runner}
HOME_DIR=$(getent passwd "$RUNNER_USER" | cut -d: -f6)
[ -n "$HOME_DIR" ] || { echo "no user $RUNNER_USER"; exit 1; }

for _ in $(seq 1 120); do   # up to 2 hours
  pgrep -f Runner.Worker >/dev/null 2>&1 || break
  sleep 60
done
if pgrep -f Runner.Worker >/dev/null 2>&1; then echo "A job is still running; try again next time."; exit 0; fi

echo "Disk before:"; df -h / | tail -1
if command -v docker >/dev/null 2>&1; then
  docker container prune -f --filter until=24h >/dev/null 2>&1 || true
  docker image prune -af --filter until=168h >/dev/null 2>&1 || true
  docker builder prune -af --filter until=168h >/dev/null 2>&1 || true
  docker volume prune -f >/dev/null 2>&1 || true
fi
old() { [ -d "$1" ] && find "$1" -mindepth 1 -maxdepth 1 -mtime +30 -exec rm -rf {} + 2>/dev/null; true; }
old "$HOME_DIR/.cargo/registry/cache"
old "$HOME_DIR/.cargo/registry/src"
old "$HOME_DIR/.cargo/git/checkouts"
old "$HOME_DIR/.gradle/caches"
old "$HOME_DIR/.gradle/wrapper/dists"
old "$HOME_DIR/.cache/ms-playwright"
old "$HOME_DIR/.cache/pip"
old "$HOME_DIR/.npm/_cacache"
for w in "$HOME_DIR"/actions-runner*/_work; do
  [ -d "$w/_temp" ] && find "$w/_temp" -mindepth 1 -maxdepth 1 -mtime +1 -exec rm -rf {} + 2>/dev/null
  # Build output from a workspace that hasn't run for 2 weeks.
  find "$w" -mindepth 3 -maxdepth 4 -type d -name target -mtime +14 -prune -exec rm -rf {} + 2>/dev/null
done
rm -f /tmp/outhouse.jks /tmp/aligned.apk
echo "Disk after:"; df -h / | tail -1
exit 0
