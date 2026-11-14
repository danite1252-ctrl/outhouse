#!/usr/bin/env bash
# DAST: OWASP ZAP against a running test server (CI's "dast" job).
#   1. baseline scan (spider + passive checks) of the site, signed out,
#      starting from an invite page (it links to the landing page and /app);
#   2. API scan (active: injection, XSS, traversal...) of the main API
#      endpoints with a signed-in user's token (ci/zap-openapi.json).
# High-risk alerts fail the job unless ci/zap-rules.tsv says why they're
# false positives, and so does any request ZAP makes the server fail (500). Everything found is printed as a table.
#   BASE=http://127.0.0.1:8080 bash ci/zap.sh
set -euo pipefail
BASE=${BASE:-http://127.0.0.1:8080}
IMAGE=${ZAP_IMAGE:-ghcr.io/zaproxy/zaproxy:stable}
WRK=$(mktemp -d /tmp/zap.XXXX)
chmod 777 "$WRK"
cp ci/zap-rules.tsv "$WRK/rules.tsv"

j() { # j METHOD PATH [JSON] [TOKEN]
  curl -fsS -X "$1" -H "Content-Type: application/json" ${4:+-H "Authorization: Bearer $4"} ${3:+-d "$3"} "$BASE$2"
}
echo "== test data for the scan"
USER_JSON=$(j POST /api/auth/register '{"username":"zapper","email":"zapper@example.test","password":"zap-password-123","display_name":"Zap","birth_date":"1990-01-01"}' "")
TOKEN=$(jq -r .token <<<"$USER_JSON"); ME=$(jq -r .user.id <<<"$USER_JSON")
GUILD=$(j POST /api/guilds '{"name":"ZAP test"}' "$TOKEN" | jq -r .id)
CHANNEL=$(j GET "/api/guilds/$GUILD/channels" "" "$TOKEN" | jq -r '[.[] | select(.kind == 0)][0].id')
MESSAGE=$(j POST "/api/channels/$CHANNEL/messages" '{"content":"hello from zap https://example.com"}' "$TOKEN" | jq -r .id)
CODE=$(j POST "/api/guilds/$GUILD/invites" '{"max_age_secs":0}' "$TOKEN" | jq -r .code)
echo "guild $GUILD, channel $CHANNEL, message $MESSAGE, invite $CODE"
sed -e "s#__BASE__#$BASE#g" -e "s#__GUILD__#$GUILD#g" -e "s#__CHANNEL__#$CHANNEL#g" \
    -e "s#__MESSAGE__#$MESSAGE#g" -e "s#__CODE__#$CODE#g" -e "s#__USER__#$ME#g" \
    ci/zap-openapi.json > "$WRK/openapi.json"
jq -e . "$WRK/openapi.json" >/dev/null

docker pull -q "$IMAGE" >/dev/null
zap() { docker run --rm --network host -v "$WRK:/zap/wrk:rw" "$@"; }

echo "== ZAP baseline (passive) from $BASE/invite/$CODE"
zap "$IMAGE" zap-baseline.py -t "$BASE/invite/$CODE" -m 2 -c rules.tsv -I \
  -J baseline.json -r baseline.html -z "-config spider.maxDuration=2" || true

echo "== ZAP API scan (active, signed in) of $(jq '.paths | length' "$WRK/openapi.json") endpoints"
zap -e ZAP_AUTH_HEADER_VALUE="Bearer $TOKEN" -e ZAP_AUTH_HEADER_SITE=127.0.0.1 \
  "$IMAGE" zap-api-scan.py -t /zap/wrk/openapi.json -f openapi -c rules.tsv -I \
  -J api.json -r api.html \
  -z "-config scanner.maxScanDurationInMins=6 -config scanner.maxRuleDurationInMins=1 -config scanner.threadPerHost=4" || true

# Summary, and fail on High alerts that aren't explained in zap-rules.tsv.
python3 - "$WRK" ci/zap-rules.tsv <<'PY'
import json, os, sys
wrk, rules_file = sys.argv[1], sys.argv[2]
accepted = {}
for line in open(rules_file):
    if line.strip() and not line.startswith("#"):
        parts = line.rstrip("\n").split("\t")
        if len(parts) >= 2 and parts[1] in ("IGNORE", "INFO", "WARN"):
            accepted[parts[0]] = parts[2] if len(parts) > 2 else ""
risk = {"0": "Info", "1": "Low", "2": "Medium", "3": "High"}
failed = []
seen_any = False
for name in ("baseline.json", "api.json"):
    path = os.path.join(wrk, name)
    if not os.path.exists(path):
        print(f"!! {name} is missing: the scan didn't finish"); failed.append(name); continue
    seen_any = True
    report = json.load(open(path))
    print(f"\n---- {name} ----")
    rows = []
    for site in report.get("site", []):
        for a in site.get("alerts", []):
            rows.append((a["riskcode"], a["pluginid"], a["alert"], int(a.get("count", len(a.get("instances", []))))))
    rows.sort(key=lambda r: (-int(r[0]), r[1]))
    for code, pid, alert, count in rows:
        note = "  (accepted: " + accepted[pid] + ")" if pid in accepted else ""
        print(f"{risk.get(code, code):7} {pid:>6}  {alert} x{count}{note}")
        if code == "3" and pid not in accepted:
            failed.append(f"{pid} {alert}")
        # ZAP's fuzzing made the server fail (500): a bug, whatever the risk level.
        if alert.startswith("A Server Error response code"):
            failed.append(f"{pid} {alert} (see the server log)")
    if not rows:
        print("no alerts")
if failed:
    print("\nZAP found High-risk problems:", *failed, sep="\n  ")
    sys.exit(1)
print("\nZAP: no unexplained High-risk alerts.")
PY
