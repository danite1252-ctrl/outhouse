#!/usr/bin/env bash
# Outhouse security check-up for your own sites. Run it on the Outhouse VM
# (it has Docker):
#   cd ~/outhouse/deploy && ./security-scan.sh                 # oh.hartspace.cc + hartspace.cc
#   ./security-scan.sh oh.hartspace.cc                         # just one site
#   ./security-scan.sh --quick                                 # skip the slow bits (ZAP, full port scan)
#   ./security-scan.sh --public-ip 203.0.113.7                 # if the address can't be worked out
#
# What it does (all of it read-only and polite: no password guessing, no
# floods, no attacks; ZAP runs its passive "baseline" scan only):
#   * TLS set-up of each site (testssl.sh, in Docker)
#   * security headers, http -> https redirects and HSTS (curl)
#   * files and folders that must never be public (.env, .git, backups,
#     server-status, folder listings, /updates/ listing...)
#   * open ports on your public address (nmap, in Docker): top 1000 TCP and
#     the UDP ports that matter, plus a direct check that Postgres (5432),
#     LiveKit's API (7880) and Outhouse itself (8080) aren't reachable
#   * OWASP ZAP baseline scan of each site (in Docker)
# Results: ~/outhouse-security/<date-time>/report.md (+ raw/ with every
# tool's own output). A short summary is printed at the end.
#
# Scanning your public IP from inside your home network goes through your
# router's "hairpin" NAT: some routers then show the LAN view (more ports
# open than the internet sees), some show nothing. For the real outside
# view, run deploy/security-scan.ps1 on a laptop tethered to a phone
# hotspot (or use an online port scanner against your public IP).
set -uo pipefail

QUICK=0
PUBLIC_IP=""
DOMAINS=()
while [ $# -gt 0 ]; do
  case $1 in
    --quick) QUICK=1 ;;
    --public-ip) PUBLIC_IP=${2:-}; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    -*) echo "Unknown option $1 (try --help)"; exit 2 ;;
    *) DOMAINS+=("$1") ;;
  esac
  shift
done
[ ${#DOMAINS[@]} -gt 0 ] || DOMAINS=(oh.hartspace.cc hartspace.cc)

STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$HOME/outhouse-security/$STAMP"
RAW="$OUT/raw"
mkdir -p "$RAW"
chmod 777 "$RAW"   # the scanners' containers run as their own users
REPORT="$OUT/report.md"
PASS=0; WARN=0; FAIL=0
FAILS=(); WARNS=()

DOCKER=docker
if ! docker info >/dev/null 2>&1; then
  if sudo -n docker info >/dev/null 2>&1 || sudo docker info >/dev/null 2>&1; then DOCKER="sudo docker"; else DOCKER=""; fi
fi

md() { printf '%s\n' "$*" >> "$REPORT"; }
ok()   { PASS=$((PASS + 1)); md "- ✅ $*"; }
warn() { WARN=$((WARN + 1)); WARNS+=("$*"); md "- ⚠️ $*"; }
bad()  { FAIL=$((FAIL + 1)); FAILS+=("$*"); md "- ❌ $*"; }
say()  { printf '\n\033[1;33m%s\033[0m\n' "$*"; }
safe_name() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'; }

{
  echo "# Outhouse security check-up, $(date '+%Y-%m-%d %H:%M %Z')"
  echo
  echo "Sites: ${DOMAINS[*]}  ·  run on $(hostname) ($(hostname -I 2>/dev/null | awk '{print $1}'))"
  echo
  echo "✅ fine · ⚠️ worth a look · ❌ fix this. Raw tool output is in \`raw/\`."
} > "$REPORT"

# ---------------------------------------------------------------------------
say "Working out the public address..."
if [ -z "$PUBLIC_IP" ]; then
  PUBLIC_IP=$(curl -fsS -m 10 https://api.ipify.org 2>/dev/null || true)
fi
md ""
md "## Addresses"
for d in "${DOMAINS[@]}"; do
  ips=$(getent ahostsv4 "$d" 2>/dev/null | awk '{print $1}' | sort -u | tr '\n' ' ')
  md "- $d → ${ips:-(no address)}"
  if [ -n "$PUBLIC_IP" ] && [ -n "$ips" ] && ! grep -qw "$PUBLIC_IP" <<<"$ips"; then
    warn "$d resolves to $ips, not this network's public address $PUBLIC_IP (fine if that's on purpose)"
  fi
done
md "- this network's public address: ${PUBLIC_IP:-unknown (use --public-ip)}"

# ---------------------------------------------------------------------------
say "Checking security headers and redirects..."
md ""
md "## Headers and redirects"
header_of() { grep -i "^$2:" "$1" | tail -1 | cut -d: -f2- | tr -d '\r' | sed 's/^ *//'; }
for d in "${DOMAINS[@]}"; do
  n=$(safe_name "$d")
  md ""
  md "### $d"
  # http -> https
  code=$(curl -sS -m 15 -o /dev/null -D "$RAW/headers-http-$n.txt" -w '%{http_code} %{redirect_url}' "http://$d/" 2>>"$RAW/errors.txt")
  case $code in
    301\ https://*|308\ https://*) ok "http://$d/ redirects permanently to ${code#* }" ;;
    302\ https://*|307\ https://*) warn "http://$d/ redirects to https, but temporarily ($code); use a permanent (301) redirect" ;;
    000*) warn "http://$d/ didn't answer (port 80 closed?): fine if you don't need it, but certificate renewals over http need it" ;;
    *) bad "http://$d/ doesn't redirect to https ($code)" ;;
  esac
  # Outhouse sites get their app and API checked too.
  paths=(/)
  if curl -fsS -m 15 "https://$d/api/health" 2>/dev/null | grep -q '"uptime_seconds"'; then
    paths=(/ /app /api/health /invite/not-a-real-code)
  fi
  for path in "${paths[@]}"; do
    f="$RAW/headers-https-$n$(safe_name "$path").txt"
    code=$(curl -sS -m 20 -o /dev/null -D "$f" -w '%{http_code}' "https://$d$path" 2>>"$RAW/errors.txt")
    if [ "${code:-000}" = 000 ]; then bad "https://$d$path didn't answer"; continue; fi
    md "- https://$d$path → $code"
    hsts=$(header_of "$f" strict-transport-security)
    if [ -z "$hsts" ]; then bad "$d$path: no Strict-Transport-Security (HSTS)"
    else
      age=$(sed -n 's/.*max-age=\([0-9]*\).*/\1/p' <<<"$hsts")
      if [ "${age:-0}" -ge 15552000 ]; then ok "$d$path: HSTS $hsts"; else warn "$d$path: HSTS max-age is short ($hsts); use at least 15552000 (180 days)"; fi
    fi
    csp=$(header_of "$f" content-security-policy)
    if [ -z "$csp" ]; then warn "$d$path: no Content-Security-Policy"
    elif grep -q "script-src[^;]*'unsafe-inline'\|script-src[^;]*'unsafe-eval'" <<<"$csp"; then warn "$d$path: CSP allows inline/eval scripts"
    else ok "$d$path: Content-Security-Policy set"; fi
    xfo=$(header_of "$f" x-frame-options)
    if [ -n "$xfo" ] || grep -q "frame-ancestors" <<<"$csp"; then ok "$d$path: framing restricted (${xfo:-frame-ancestors})"; else warn "$d$path: no X-Frame-Options / frame-ancestors (clickjacking)"; fi
    if grep -qi '^x-content-type-options: *nosniff' "$f"; then ok "$d$path: X-Content-Type-Options nosniff"; else warn "$d$path: no X-Content-Type-Options: nosniff"; fi
    refpol=$(header_of "$f" referrer-policy)
    if [ -n "$refpol" ]; then ok "$d$path: Referrer-Policy $refpol"; else warn "$d$path: no Referrer-Policy"; fi
    server=$(header_of "$f" server)
    if grep -q '[0-9]\.[0-9]\|(Ubuntu)\|(Debian)' <<<"$server"; then warn "$d$path: Server header gives away versions ($server): ServerTokens Prod"; fi
    powered=$(header_of "$f" x-powered-by)
    if [ -n "$powered" ]; then warn "$d$path: X-Powered-By: $powered (remove it)"; fi
    if grep -qi '^set-cookie:' "$f"; then
      if grep -i '^set-cookie:' "$f" | grep -vqi 'secure'; then warn "$d$path: a cookie without the Secure flag"; fi
      if grep -i '^set-cookie:' "$f" | grep -vqi 'httponly'; then warn "$d$path: a cookie without HttpOnly"; fi
    fi
  done
  # TRACE should be off.
  tcode=$(curl -sS -m 15 -o /dev/null -w '%{http_code}' -X TRACE "https://$d/" 2>/dev/null)
  case ${tcode:-000} in
    000) ;;
    200) bad "$d answers HTTP TRACE (TraceEnable Off)" ;;
    *) ok "$d: TRACE refused ($tcode)" ;;
  esac
done

# ---------------------------------------------------------------------------
say "Looking for files that shouldn't be public..."
md ""
md "## Things that must not be public"
PATHS=(/.env /.env.bak /.git/HEAD /.git/config /.gitignore /.htaccess /.htpasswd /.DS_Store
  /deploy/ /deploy/.env /docker-compose.yml /Dockerfile /Cargo.toml /backup.sql /dump.sql /db.sql
  /database.dump /outhouse-backups/ /index.php.bak /config.php.bak /wp-config.php.bak
  /server-status /server-info /phpinfo.php /info.php /updates/ /icons/ /fonts/ /files/ /web/
  /.well-known/ /livekit.yaml /twirp/livekit.RoomService/ListRooms)
for d in "${DOMAINS[@]}"; do
  n=$(safe_name "$d")
  md ""
  md "### $d"
  # What a page that doesn't exist looks like (some sites answer 200 for everything).
  curl -sS -m 15 -o "$RAW/404-$n.html" "https://$d/no-such-page-$RANDOM$RANDOM" 2>/dev/null || true
  touch "$RAW/404-$n.html"
  soft404=$(md5sum < "$RAW/404-$n.html" | cut -d' ' -f1)
  found=0
  for p in "${PATHS[@]}"; do
    body="$RAW/probe-$n$(safe_name "$p")"
    code=$(curl -sS -m 15 -o "$body" -w '%{http_code}' "https://$d$p" 2>/dev/null)
    if [ "$code" = 200 ]; then
      if [ "$(md5sum < "$body" | cut -d' ' -f1)" = "$soft404" ]; then rm -f "$body"; continue; fi
      if grep -qi '<title>Index of\|Directory listing for' "$body"; then bad "$d$p is a folder listing"; found=1
      elif [ "$p" = /twirp/livekit.RoomService/ListRooms ]; then :
      elif grep -qi '<html' "$body" && ! grep -qiE 'apache status|server information|phpinfo|\[core\]|ref: refs' "$body"; then
        # An HTML page (the app shell, a login page): probably harmless.
        md "- $d$p → 200 (an ordinary page; check \`raw/\` if unsure)"; continue
      else bad "$d$p is publicly readable (HTTP 200) — check raw/$(basename "$body")"; found=1; fi
    else
      rm -f "$body"
    fi
  done
  if [ $found = 0 ]; then ok "$d: none of ${#PATHS[@]} sensitive paths are exposed"; fi
done

# ---------------------------------------------------------------------------
if [ -z "$DOCKER" ]; then
  warn "Docker isn't available, so TLS, port and ZAP scans were skipped"
else
  say "Checking TLS with testssl.sh (a few minutes per site)..."
  md ""
  md "## TLS (testssl.sh)"
  for d in "${DOMAINS[@]}"; do
    n=$(safe_name "$d")
    $DOCKER run --rm -v "$RAW:/data" drwetter/testssl.sh --quiet --color 0 --warnings off \
      --jsonfile "/data/testssl-$n.json" --logfile "/data/testssl-$n.log" \
      "https://$d" >/dev/null 2>&1 || true
    if [ ! -s "$RAW/testssl-$n.json" ]; then warn "$d: testssl.sh didn't produce results (see raw/)"; continue; fi
    summary=$(python3 - "$RAW/testssl-$n.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
items = data if isinstance(data, list) else data.get("scanResult", [])
if isinstance(items, list) and items and "serverDefaults" in (items[0] if isinstance(items[0], dict) else {}):
    flat = []
    for r in items:
        for k, v in r.items():
            if isinstance(v, list): flat += v
    items = flat
bad = [i for i in items if isinstance(i, dict) and i.get("severity") in ("HIGH", "CRITICAL")]
med = [i for i in items if isinstance(i, dict) and i.get("severity") == "MEDIUM"]
for i in bad: print("BAD", i.get("id"), "-", i.get("finding"))
for i in med: print("MED", i.get("id"), "-", i.get("finding"))
for i in items:
    if isinstance(i, dict) and i.get("id") in ("overall_grade", "cert_notAfter", "TLS1", "TLS1_1", "TLS1_3"):
        print("INFO", i.get("id"), "-", i.get("finding"))
PY
)
    grade=$(sed -n 's/^INFO overall_grade - //p' <<<"$summary")
    md "- $d: grade ${grade:-?}"
    while IFS= read -r line; do
      case $line in
        BAD*) bad "$d TLS: ${line#BAD }" ;;
        MED*) warn "$d TLS: ${line#MED }" ;;
        INFO*) md "  - ${line#INFO }" ;;
      esac
    done <<<"$summary"
    if ! grep -q '^BAD' <<<"$summary"; then ok "$d: no high-severity TLS problems"; fi
  done

  if [ -n "$PUBLIC_IP" ]; then
    say "Scanning ports on $PUBLIC_IP (nmap; top 1000 TCP + key UDP)..."
    md ""
    md "## Open ports on $PUBLIC_IP"
    md "(From inside your network this may show the LAN view through the router's hairpin NAT; see the note at the top of this script.)"
    TCP_PORTS="--top-ports 1000"
    [ $QUICK = 1 ] && TCP_PORTS="-p 21,22,23,25,53,80,110,143,443,445,3306,3389,5432,5900,6379,7880,7881,8080,8443,9000,27017"
    # shellcheck disable=SC2086
    $DOCKER run --rm --net host -v "$RAW:/data" instrumentisto/nmap -Pn -sS $TCP_PORTS -T4 --open \
      -oN /data/nmap-tcp.txt -oX /data/nmap-tcp.xml "$PUBLIC_IP" >/dev/null 2>&1 || true
    $DOCKER run --rm --net host -v "$RAW:/data" instrumentisto/nmap -Pn -sU -p 53,123,161,500,1900,3478,4500,5353,7882,11211,51820 \
      -T4 -oN /data/nmap-udp.txt "$PUBLIC_IP" >/dev/null 2>&1 || true
    expected_tcp=" 80 443 7881 "
    if [ -s "$RAW/nmap-tcp.txt" ]; then
      while read -r portproto _ service; do
        port=${portproto%/*}
        if [[ $expected_tcp == *" $port "* ]]; then ok "TCP $port open ($service) — expected"
        elif [ "$port" = 22 ]; then warn "TCP 22 (SSH) is open to the internet: keys only (PasswordAuthentication no) and fail2ban, or close it on the router and use a VPN"
        else bad "TCP $port open ($service) — not expected; close it on the router/firewall unless you know why it's there"; fi
      done < <(grep -E '^[0-9]+/tcp +open' "$RAW/nmap-tcp.txt")
    else
      warn "nmap TCP scan didn't run (see raw/)"
    fi
    if [ -s "$RAW/nmap-udp.txt" ]; then
      while read -r portproto state service; do
        port=${portproto%/*}
        if [ "$port" = 7882 ]; then ok "UDP 7882 $state ($service) — voice, expected"
        elif [ "$state" = open ]; then warn "UDP $port open ($service): expected?"; fi
      done < <(grep -E '^[0-9]+/udp +open' "$RAW/nmap-udp.txt")
      md "  (UDP 'open|filtered' just means no answer; see raw/nmap-udp.txt)"
    fi
    # The ones that matter most, checked directly.
    for port in 5432 7880 8080 6379 2375; do
      if timeout 5 bash -c "exec 3<>/dev/tcp/$PUBLIC_IP/$port" 2>/dev/null; then
        bad "Port $port answers on the public address (Postgres 5432 / LiveKit API 7880 / Outhouse 8080 / Redis 6379 / Docker API 2375 must stay private)"
      else
        ok "Port $port isn't reachable on the public address"
      fi
    done
  else
    warn "No public address known, so no port scan (use --public-ip)"
  fi

  if [ $QUICK = 0 ]; then
    say "Running the OWASP ZAP baseline scan (passive, a few minutes per site)..."
    md ""
    md "## OWASP ZAP baseline (passive)"
    for d in "${DOMAINS[@]}"; do
      n=$(safe_name "$d")
      $DOCKER run --rm -v "$RAW:/zap/wrk:rw" ghcr.io/zaproxy/zaproxy:stable zap-baseline.py \
        -t "https://$d/" -m 2 -I -J "zap-$n.json" -r "zap-$n.html" >"$RAW/zap-$n.log" 2>&1 || true
      if [ ! -s "$RAW/zap-$n.json" ]; then warn "$d: ZAP didn't finish (see raw/zap-$n.log)"; continue; fi
      python3 - "$RAW/zap-$n.json" "$d" <<'PY' >> "$RAW/zap-summary.txt"
import json, sys
risk = {"0": "Info", "1": "Low", "2": "Medium", "3": "High"}
r = json.load(open(sys.argv[1]))
for site in r.get("site", []):
    for a in sorted(site.get("alerts", []), key=lambda a: -int(a["riskcode"])):
        print(f'{sys.argv[2]}\t{risk.get(a["riskcode"], "?")}\t{a["pluginid"]}\t{a["alert"]}\t{a.get("count", "")}')
PY
      highs=$(awk -F'\t' -v d="$d" '$1 == d && $2 == "High"' "$RAW/zap-summary.txt" | wc -l)
      meds=$(awk -F'\t' -v d="$d" '$1 == d && $2 == "Medium"' "$RAW/zap-summary.txt" | wc -l)
      md "- $d: $highs high, $meds medium (full report: raw/zap-$n.html)"
      awk -F'\t' -v d="$d" '$1 == d && ($2 == "High" || $2 == "Medium") {print "  - " $2 ": " $4 " (" $3 ")"}' "$RAW/zap-summary.txt" >> "$REPORT"
      if [ "$highs" -gt 0 ]; then bad "$d: ZAP found $highs high-risk issue(s)"; else ok "$d: ZAP found no high-risk issues"; fi
    done
  fi
fi

# ---------------------------------------------------------------------------
md ""
md "## Summary"
md "$PASS fine, $WARN worth a look, $FAIL to fix."
{
  md ""
  md "Next steps: deploy/README.md → \"Security checks\"; Apache settings in deploy/apache-hardening.conf."
}
say "Done: $PASS fine, $WARN worth a look, $FAIL to fix."
for f in "${FAILS[@]}"; do printf '  \033[31m✗\033[0m %s\n' "$f"; done
for w in "${WARNS[@]}"; do printf '  \033[33m!\033[0m %s\n' "$w"; done
echo
echo "Report: $REPORT"
echo "Raw output: $RAW"
[ "$FAIL" = 0 ]
