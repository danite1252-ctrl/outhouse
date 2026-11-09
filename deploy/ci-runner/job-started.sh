#!/usr/bin/env bash
# Runner hook (ACTIONS_RUNNER_HOOK_JOB_STARTED, set in the runner's .env by
# setup-linux-runner.sh). Runs before every job, before GitHub starts the
# job's service containers, so it can also clear a stale Postgres left by a
# job that was killed half-way (it would hold port 5432 and the next job's
# Postgres couldn't start). The workflow's own steps do the rest
# (ci/runner-cleanup.sh). Never fails the job.
set -u
if command -v docker >/dev/null 2>&1; then
  docker rm -f livekit >/dev/null 2>&1 || true
  docker ps -aq --filter ancestor=ghcr.io/zaproxy/zaproxy:stable | xargs -r docker rm -f >/dev/null 2>&1 || true
  # Anything still publishing Postgres' port (no job is running yet, so it's stale).
  docker ps -q --filter publish=5432 | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker network prune -f >/dev/null 2>&1 || true
fi
for p in 'target/debug/outhouse-server' 'ci/wow_mock.py' 'ci/livekit_mock.py' 'ci/push_mock.py' \
         'ci/gateway_probe.mjs' 'web/bots/dice-bot.mjs'; do
  pkill -f "$p" 2>/dev/null || true
done
if command -v fuser >/dev/null 2>&1; then
  for port in 8080 8097 8098 8099 7880 7881; do fuser -k -n tcp "$port" >/dev/null 2>&1 || true; done
  fuser -k -n udp 7882 >/dev/null 2>&1 || true
fi
rm -f /tmp/outhouse.jks /tmp/aligned.apk
rm -rf /tmp/outhouse-uploads /tmp/ui-shots /tmp/zap.* /tmp/wh /tmp/package 2>/dev/null || true
echo "outhouse runner: cleared leftovers before the job"
exit 0
