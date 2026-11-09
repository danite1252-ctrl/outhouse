#!/usr/bin/env bash
# Clears what a CI job leaves behind, so the next job starts clean on a
# self-hosted runner (the same machine runs job after job). Harmless on
# GitHub's runners, which are thrown away after every job.
#
#   bash ci/runner-cleanup.sh          # start of a job: also clears old screenshots/uploads
#   bash ci/runner-cleanup.sh --end    # end of a job (if: always()): same, keeps nothing secret
#
# It never touches the job's own service containers (Postgres on 5432):
# GitHub starts and removes those itself. deploy/ci-runner/job-started.sh
# (a runner hook, runs before services start) also clears stale ones.
set -u
MODE=${1:-}

# Named containers CI starts (LiveKit for the voice tests) and ZAP scans
# (docker run --rm, but a killed job can leave them running).
if command -v docker >/dev/null 2>&1; then
  docker rm -f livekit >/dev/null 2>&1 || true
  ZAP_IMAGE=${ZAP_IMAGE:-ghcr.io/zaproxy/zaproxy:stable}
  docker ps -aq --filter "ancestor=$ZAP_IMAGE" 2>/dev/null | xargs -r docker rm -f >/dev/null 2>&1 || true
fi

# Test server, mocks, bot and gateway probe from earlier runs.
for p in 'target/debug/outhouse-server' 'ci/wow_mock.py' 'ci/livekit_mock.py' 'ci/push_mock.py' \
         'ci/gateway_probe.mjs' 'web/bots/dice-bot.mjs'; do
  pkill -f "$p" 2>/dev/null || true
done
# Anything else still holding the ports CI uses: server 8080, mocks 8097-8099,
# LiveKit 7880-7882 (host network). Not 5432: that's the job's Postgres service.
if command -v fuser >/dev/null 2>&1; then
  for port in 8080 8097 8098 8099 7880 7881; do fuser -k -n tcp "$port" >/dev/null 2>&1 || true; done
  fuser -k -n udp 7882 >/dev/null 2>&1 || true
fi

# Temporary files and folders (the Android signing key first: always gone).
rm -f /tmp/outhouse.jks /tmp/aligned.apk
rm -rf /tmp/outhouse-uploads /tmp/zap.* /tmp/wh /tmp/package /tmp/livekit-client-*.tgz \
       /tmp/update_state_tests /tmp/apk-manifest.txt /tmp/audit-*.txt \
       /tmp/body /tmp/pixel.png /tmp/fake.png /tmp/big.png /tmp/emoji.png /tmp/av.png /tmp/av-headers \
       /tmp/gw-*.jsonl /tmp/entrance-test.wav 2>/dev/null || true
rm -f /tmp/outhouse-server.log /tmp/ui-server.log /tmp/dast-server.log /tmp/wow-mock.log \
      /tmp/wow-mock-ui.log /tmp/livekit-mock.log /tmp/push-mock.log /tmp/bot.log 2>/dev/null || true
# Screenshots are uploaded before the end-of-job cleanup runs.
rm -rf /tmp/ui-shots 2>/dev/null || true

[ "$MODE" = --end ] && echo "Cleaned up after the job." || echo "Cleared leftovers from earlier runs."
exit 0
