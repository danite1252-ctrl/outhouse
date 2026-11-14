#!/usr/bin/env bash
# Email, password resets and two-factor sign-in, end to end against a real
# server and Postgres, with the SMTP sink (ci/smtp_sink.py) catching mail.
# ci/smoke.sh runs it near its end; it can also run on its own:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=... MAIL_DIR=/tmp/outhouse-mail \
#   OWNER_TOKEN=<the site owner's token> bash ci/auth_test.sh
#
# The server must have been started with SMTP_HOST/SMTP_PORT pointing at the
# sink, SMTP_TLS=none, MAIL_FROM, PUBLIC_URL=$BASE and AUTH_SECRET_KEY.
# With no OWNER_TOKEN, the database must be new (the first account made
# here becomes the site owner). OUTHOUSE_TEST_HOOKS=1 too (ip-blocks/reload).
set -euo pipefail
trap 'echo "Auth test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
MAIL_DIR=${MAIL_DIR:-/tmp/outhouse-mail}
SERVER_BIN=${SERVER_BIN:-./target/debug/outhouse-server}
AB=/tmp/auth-body
RUN=${AUTH_RUN:-$(date +%s | tail -c 6)}
astep() { echo "== auth: $*"; }

# areq METHOD PATH [JSON] [TOKEN] [X-Forwarded-For] [extra curl args...] -> HTTP status (body in $AB)
areq() {
  local method=$1 path=$2 body=${3:-} token=${4:-} xff=${5:-}
  shift $(( $# < 5 ? $# : 5 ))
  local args=(-sS -X "$method" -o "$AB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$xff" ] && args+=(-H "X-Forwarded-For: $xff")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$@" "$BASE$path"
}
# expect STATUS METHOD PATH [JSON] [TOKEN] [XFF] [curl args] -> fails unless the status matches
expect() {
  local want=$1; shift
  local got; got=$(areq "$@")
  if [ "$got" != "$want" ]; then echo "EXPECTED $want, GOT $got: $1 $2"; cat "$AB"; echo; return 1; fi
}
body() { cat "$AB"; }

mail_n() { find "$MAIL_DIR" -name '*.json' 2>/dev/null | wc -l; }
# wait_mail TO SUBJECT_PART SINCE -> prints the file of the first such mail after number SINCE
wait_mail() {
  local to=$1 subj=$2 since=$3 f n
  for _ in $(seq 1 60); do
    for f in $(find "$MAIL_DIR" -name '*.json' 2>/dev/null | sort); do
      n=$(basename "$f" .json); n=$((10#$n))
      [ "$n" -gt "$since" ] || continue
      if jq -e --arg to "$to" --arg s "$subj" '(.to | index($to)) and (.subject | contains($s))' "$f" >/dev/null; then
        echo "$f"; return 0
      fi
    done
    sleep 0.25
  done
  echo "no mail to $to about \"$subj\" (after #$since):" >&2
  for f in $(find "$MAIL_DIR" -name '*.json' 2>/dev/null | sort | tail -5); do jq -c '{n, to, subject}' "$f" >&2; done
  return 1
}
# no_mail TO SINCE -> fails if any mail reached TO after number SINCE (waits a moment first)
no_mail() {
  sleep 1.5
  local f n
  for f in $(find "$MAIL_DIR" -name '*.json' 2>/dev/null | sort); do
    n=$(basename "$f" .json); n=$((10#$n))
    [ "$n" -gt "$2" ] || continue
    if jq -e --arg to "$1" '.to | index($to)' "$f" >/dev/null; then echo "unexpected mail to $1:"; jq -c '{to, subject}' "$f"; return 1; fi
  done
}
link_token() { jq -r .text "$1" | grep -oE 'token=[0-9a-f]{64}' | head -1 | cut -d= -f2; }
totp() { python3 ci/totp.py "$1" "${2:-0}"; }
# register NAME EMAIL [XFF] -> token
register() {
  expect 200 POST /api/auth/register "{\"username\":\"$1\",\"email\":\"$2\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\",\"display_name\":\"$1\"}" "" "${3:-198.18.40.$((RANDOM % 200 + 1))}"
  jq -r .token "$AB"
}
uid_of() { psql "$DATABASE_URL" -tAc "SELECT id FROM users WHERE username = '$1'"; }

# The site owner (first account) when run on its own.
if [ -z "${OWNER_TOKEN:-}" ]; then
  OWNER_TOKEN=$(register "owner$RUN" "owner$RUN@example.test")
fi

astep "sign-up needs an email (checked, stored lower-case)"
expect 400 POST /api/auth/register "{\"username\":\"noemail$RUN\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\"}" "" 198.18.40.1
grep -q "Add an email address" "$AB"
expect 400 POST /api/auth/register "{\"username\":\"bademail$RUN\",\"email\":\"not-an-email\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\"}" "" 198.18.40.1
grep -q "doesn't look like an email" "$AB"
expect 200 GET /api/auth/registration
jq -e '.email_available == true' "$AB" >/dev/null
M0=$(mail_n)
VERA_MAIL="vera$RUN@example.test"
VERA=$(register "vera$RUN" "Vera$RUN@Example.TEST")
[ "$(psql "$DATABASE_URL" -tAc "SELECT email FROM users WHERE username = 'vera$RUN'")" = "$VERA_MAIL" ]
expect 200 GET /api/users/@me "" "$VERA"
jq -e --arg e "$VERA_MAIL" '.email == $e and .email_verified == false and .email_missing == false and .email_available == true
  and .two_factor == false and .two_factor_nudge == "gentle"' "$AB" >/dev/null

astep "the confirmation link works once"
F=$(wait_mail "$VERA_MAIL" "Confirm your email" "$M0")
jq -e '.text | contains("/verify-email?token=")' "$F" >/dev/null
jq -e '.html | (contains("<img") | not)' "$F" >/dev/null   # no tracking pixels
VT=$(link_token "$F")
[ "${#VT}" = 64 ]
# The link opens the confirmation page (no inline script, same strict CSP).
curl -fsS "$BASE/verify-email?token=$VT" -o /tmp/auth-page && grep -q 'src="/email-link.js"' /tmp/auth-page
expect 200 POST /api/auth/verify-email "{\"token\":\"$VT\"}"
expect 400 POST /api/auth/verify-email "{\"token\":\"$VT\"}"
grep -q "expired or was already used" "$AB"
expect 200 GET /api/users/@me "" "$VERA"
jq -e '.email_verified == true' "$AB" >/dev/null
expect 400 POST /api/users/@me/email/resend "" "$VERA"

astep "an address another account confirmed: sign-up looks the same, but no link (a note instead)"
M1=$(mail_n)
COPY=$(register "copycat$RUN" "VERA$RUN@example.test")
expect 200 GET /api/users/@me "" "$COPY"
jq -e --arg e "$VERA_MAIL" '.email == $e and .email_verified == false' "$AB" >/dev/null
F=$(wait_mail "$VERA_MAIL" "You already have an Outhouse account" "$M1")
! jq -e '.text | contains("token=")' "$F" >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM email_tokens WHERE user_id = $(uid_of "copycat$RUN")")" = 0 ]

astep "accounts from before email was needed see the warning, and can add one (password needed)"
OLDIE=$(register "oldie$RUN" "oldie$RUN@example.test")
psql "$DATABASE_URL" -qc "UPDATE users SET email = NULL, email_verified_at = NULL WHERE username = 'oldie$RUN'"
expect 200 GET /api/users/@me "" "$OLDIE"
jq -e '.email == null and .email_missing == true' "$AB" >/dev/null
expect 400 PUT /api/users/@me/email '{"email":"oldie2@example.test","password":"wrong-password"}' "$OLDIE"
M2=$(mail_n)
expect 200 PUT /api/users/@me/email "{\"email\":\"oldie$RUN@example.test\",\"password\":\"password123\"}" "$OLDIE"
jq -e '.email_missing == false and .email_verified == false' "$AB" >/dev/null
F=$(wait_mail "oldie$RUN@example.test" "Confirm your email" "$M2")
expect 200 POST /api/auth/verify-email "{\"token\":\"$(link_token "$F")\"}"

astep "changing a confirmed email: the old one stays until the new one is confirmed, and gets a heads-up"
M3=$(mail_n)
NEW_MAIL="vera.new$RUN@example.test"
expect 200 PUT /api/users/@me/email "{\"email\":\"$NEW_MAIL\",\"password\":\"password123\"}" "$VERA"
jq -e --arg o "$VERA_MAIL" --arg n "$NEW_MAIL" '.email == $o and .email_verified and .email_pending == $n' "$AB" >/dev/null
wait_mail "$VERA_MAIL" "being changed" "$M3" >/dev/null
F=$(wait_mail "$NEW_MAIL" "Confirm your email" "$M3")
expect 200 POST /api/auth/verify-email "{\"token\":\"$(link_token "$F")\"}"
expect 200 GET /api/users/@me "" "$VERA"
jq -e --arg n "$NEW_MAIL" '.email == $n and .email_verified and .email_pending == null' "$AB" >/dev/null
VERA_MAIL=$NEW_MAIL
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = 'email_changed' AND target_user_id = $(uid_of "vera$RUN")")" -ge 2 ]

astep "forgot password: always the same answer; only confirmed addresses get a link"
NEUTRAL="If that account has a confirmed email, we've sent a link."
M4=$(mail_n)
expect 200 POST /api/auth/forgot '{"login":"nobody-at-all"}' "" 198.18.41.1
jq -e --arg m "$NEUTRAL" '.sent == true and (.message | startswith($m))' "$AB" >/dev/null
expect 200 POST /api/auth/forgot "{\"login\":\"copycat$RUN\"}" "" 198.18.41.1
jq -e --arg m "$NEUTRAL" '.message | startswith($m)' "$AB" >/dev/null
expect 200 POST /api/auth/forgot "{\"login\":\"$VERA_MAIL\"}" "" 198.18.41.1
jq -e --arg m "$NEUTRAL" '.message | startswith($m)' "$AB" >/dev/null
F1=$(wait_mail "$VERA_MAIL" "Reset your Outhouse password" "$M4")
T1=$(link_token "$F1")
no_mail "vera$RUN@example.test" "$M4"   # the copycat's (unconfirmed) address got nothing
M5=$(mail_n)
expect 200 POST /api/auth/forgot "{\"login\":\"VERA$RUN\"}" "" 198.18.41.1
F2=$(wait_mail "$VERA_MAIL" "Reset your Outhouse password" "$M5")
T2=$(link_token "$F2")
[ "$T1" != "$T2" ]
# A newer link replaces the older one.
expect 400 POST /api/auth/reset/check "{\"token\":\"$T1\"}"
# Per address: 5 an hour.
expect 200 POST /api/auth/forgot '{"login":"nobody-at-all"}' "" 198.18.41.1
expect 429 POST /api/auth/forgot '{"login":"nobody-at-all"}' "" 198.18.41.1

astep "reset: the page, the rules, one use, everyone signed out, a notice by email"
curl -fsS "$BASE/reset?token=$T2" -o /tmp/auth-page && grep -q 'name="referrer" content="no-referrer"' /tmp/auth-page
expect 200 POST /api/auth/reset/check "{\"token\":\"$T2\"}"
jq -e --arg u "vera$RUN" '.ok and .username == $u and .two_factor == false' "$AB" >/dev/null
expect 400 POST /api/auth/reset "{\"token\":\"$T2\",\"password\":\"short\"}"
grep -q "at least 8" "$AB"
VERA_OTHER=$(TOKEN="" areq POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"password123\"}" >/dev/null; jq -r .token "$AB")
M6=$(mail_n)
expect 200 POST /api/auth/reset "{\"token\":\"$T2\",\"password\":\"new-password-1\"}"
jq -e --arg u "vera$RUN" '.ok and .username == $u and .suspended == false' "$AB" >/dev/null
expect 400 POST /api/auth/reset "{\"token\":\"$T2\",\"password\":\"new-password-2\"}"
expect 401 GET /api/users/@me "" "$VERA"
expect 401 GET /api/users/@me "" "$VERA_OTHER"
wait_mail "$VERA_MAIL" "Your Outhouse password was changed" "$M6" >/dev/null
expect 401 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"password123\"}" "" 198.18.41.2
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.41.2
VERA=$(jq -r .token "$AB")
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = 'password_reset' AND target_user_id = $(uid_of "vera$RUN")")" = 1 ]
# Links run out (30 minutes).
M7=$(mail_n)
expect 200 POST /api/auth/forgot "{\"login\":\"vera$RUN\"}" "" 198.18.41.3
T3=$(link_token "$(wait_mail "$VERA_MAIL" "Reset your Outhouse password" "$M7")")
psql "$DATABASE_URL" -qc "UPDATE email_tokens SET expires_at = now() - interval '1 minute' WHERE token_hash = encode(sha256('$T3'::bytea), 'hex')"
expect 400 POST /api/auth/reset/check "{\"token\":\"$T3\"}"
expect 400 POST /api/auth/reset "{\"token\":\"$T3\",\"password\":\"new-password-3\"}"
# Junk tokens.
expect 400 POST /api/auth/reset/check '{"token":"nope"}'
# Per account: after 5 an hour it quietly stops sending (the answer stays the same).
M8=$(mail_n)
for i in 1 2 3 4 5; do expect 200 POST /api/auth/forgot "{\"login\":\"oldie$RUN\"}" "" "198.18.41.$((10 + i))"; done
wait_mail "oldie$RUN@example.test" "Reset your Outhouse password" "$M8" >/dev/null
sleep 1
M8=$(mail_n)
expect 200 POST /api/auth/forgot "{\"login\":\"oldie$RUN\"}" "" 198.18.41.20
jq -e --arg m "$NEUTRAL" '.message | startswith($m)' "$AB" >/dev/null
no_mail "oldie$RUN@example.test" "$M8"

astep "reset requests from a blocked network are refused, like sign-in"
BLOCK_ID=$((900000000000 + RUN))
psql "$DATABASE_URL" -qc "INSERT INTO ip_blocks (id, ip, reason) VALUES ($BLOCK_ID, '198.51.100.250/32', 'auth test')"
expect 200 POST /api/safety/ip-blocks/reload "" "$OWNER_TOKEN"
expect 403 POST /api/auth/forgot "{\"login\":\"vera$RUN\"}" "" 198.51.100.250
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM blocked_attempts WHERE block_id = $BLOCK_ID AND kind = 'reset'")" -ge 1 ]
psql "$DATABASE_URL" -qc "UPDATE ip_blocks SET removed_at = now() WHERE id = $BLOCK_ID"
expect 200 POST /api/safety/ip-blocks/reload "" "$OWNER_TOKEN"

astep "two-factor: set up (password, then a code), recovery codes once"
expect 400 POST /api/auth/2fa/setup '{"password":"wrong"}' "$VERA"
expect 200 POST /api/auth/2fa/setup '{"password":"new-password-1"}' "$VERA"
SECRET=$(jq -r .secret "$AB")
jq -e --arg u "vera$RUN" '(.uri | startswith("otpauth://totp/Outhouse:" + $u + "?secret=")) and (.uri | contains("issuer=Outhouse"))
  and (.qr_svg | startswith("<?xml") or startswith("<svg")) and (.secret | length) == 32' "$AB" >/dev/null
# Stored encrypted, never as the plain secret.
SEALED=$(psql "$DATABASE_URL" -tAc "SELECT totp_pending FROM users WHERE username = 'vera$RUN'")
[[ "$SEALED" == v1.* ]] && [[ "$SEALED" != *"$SECRET"* ]]
expect 400 POST /api/auth/2fa/confirm '{"code":"000000"}' "$VERA"
M9=$(mail_n)
CONF_CODE=$(totp "$SECRET" -1)
expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$CONF_CODE\"}" "$VERA"
jq -e '.enabled and (.recovery_codes | length) == 10' "$AB" >/dev/null
R1=$(jq -r '.recovery_codes[0]' "$AB"); R2=$(jq -r '.recovery_codes[1]' "$AB"); R3=$(jq -r '.recovery_codes[2]' "$AB")
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM recovery_codes WHERE user_id = $(uid_of "vera$RUN") AND code_hash LIKE '%$R1%'")" = 0 ]
wait_mail "$VERA_MAIL" "Two-factor sign-in is on" "$M9" >/dev/null
expect 200 GET /api/users/@me "" "$VERA"
jq -e '.two_factor == true and .two_factor_nudge == null' "$AB" >/dev/null
expect 200 GET /api/auth/2fa "" "$VERA"
jq -e '.enabled and .recovery_codes_left == 10 and .trusted_devices == []' "$AB" >/dev/null

astep "two-factor sign-in: a ticket (not a session), replayed codes refused, trust this device"
JAR=$(mktemp)
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.1
jq -e '.mfa_required == true and (.token | not) and (.ticket | length) == 64' "$AB" >/dev/null
TICKET=$(jq -r .ticket "$AB")
expect 401 GET /api/users/@me "" "$TICKET"   # a ticket is no session
# The code used to turn it on can't be used again (that exact code: regenerating
# "one step back" here would be a fresh, valid code once a 30 s step has passed).
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$CONF_CODE\",\"trust_days\":30}" "" 198.18.42.1
CODE0=$(totp "$SECRET" 0)
expect 200 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$CODE0\",\"trust_days\":30}" "" 198.18.42.1 -c "$JAR" -D /tmp/auth-headers
VERA2=$(jq -r .token "$AB")
grep -i '^set-cookie: oh_td_' /tmp/auth-headers | grep -qi 'HttpOnly'
grep -i '^set-cookie: oh_td_' /tmp/auth-headers | grep -qi 'SameSite=Strict'
grep -i '^set-cookie: oh_td_' /tmp/auth-headers | grep -qi 'Path=/api/auth/'
expect 200 GET /api/users/@me "" "$VERA2"
# The ticket is gone, and that code is used up.
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$CODE0\"}" "" 198.18.42.1
# This device is trusted now: no code step.
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.1 -b "$JAR"
jq -e '(.token | length) == 64 and (.mfa_required | not)' "$AB" >/dev/null
expect 200 GET /api/auth/2fa "" "$VERA2" "" -b "$JAR"
jq -e '(.trusted_devices | length) == 1 and .trusted_devices[0].current == true and .trusted_devices[0].label != ""' "$AB" >/dev/null
DEVICE=$(jq -r '.trusted_devices[0].id' "$AB")
# Another device still needs the code; a recovery code works once.
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.2
TICKET=$(jq -r .ticket "$AB")
expect 200 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$R1\"}" "" 198.18.42.2
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.2
TICKET=$(jq -r .ticket "$AB")
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$R1\"}" "" 198.18.42.2
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = 'recovery_code_used' AND target_user_id = $(uid_of "vera$RUN")")" = 1 ]
expect 200 GET /api/auth/2fa "" "$VERA2"
jq -e '.recovery_codes_left == 9' "$AB" >/dev/null
# Forgetting the device brings the code step back.
expect 200 DELETE "/api/auth/2fa/devices/$DEVICE" "" "$VERA2"
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.1 -b "$JAR"
jq -e '.mfa_required == true' "$AB" >/dev/null

astep "two-factor: new recovery codes need a code; the old ones stop working"
expect 400 POST /api/auth/2fa/recovery-codes '{"code":"123456"}' "$VERA2"
expect 200 POST /api/auth/2fa/recovery-codes "{\"code\":\"$(totp "$SECRET" 1)\"}" "$VERA2"
jq -e '(.recovery_codes | length) == 10' "$AB" >/dev/null
N1=$(jq -r '.recovery_codes[0]' "$AB"); N2=$(jq -r '.recovery_codes[1]' "$AB"); N3=$(jq -r '.recovery_codes[2]' "$AB")
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-1\"}" "" 198.18.42.3
TICKET=$(jq -r .ticket "$AB")
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$R2\"}" "" 198.18.42.3

astep "reset with two-factor on needs a code too; it signs out everywhere and forgets trusted devices"
psql "$DATABASE_URL" -qc "INSERT INTO trusted_devices (id, user_id, token_hash, expires_at) VALUES ($((BLOCK_ID + 1)), $(uid_of "vera$RUN"), 'x$RUN', now() + interval '1 day')"
M10=$(mail_n)
expect 200 POST /api/auth/forgot "{\"login\":\"vera$RUN\"}" "" 198.18.43.1
T4=$(link_token "$(wait_mail "$VERA_MAIL" "Reset your Outhouse password" "$M10")")
expect 200 POST /api/auth/reset/check "{\"token\":\"$T4\"}"
jq -e '.two_factor == true' "$AB" >/dev/null
expect 400 POST /api/auth/reset "{\"token\":\"$T4\",\"password\":\"new-password-4\"}"
grep -q "6-digit code" "$AB"
expect 400 POST /api/auth/reset "{\"token\":\"$T4\",\"password\":\"new-password-4\",\"code\":\"$R3\"}"   # an old recovery code
expect 200 POST /api/auth/reset "{\"token\":\"$T4\",\"password\":\"new-password-4\",\"code\":\"$N1\"}"
expect 401 GET /api/users/@me "" "$VERA2"
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM trusted_devices WHERE user_id = $(uid_of "vera$RUN")")" = 0 ]
# Two-factor is still on after a reset.
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-4\"}" "" 198.18.43.2
jq -e '.mfa_required == true' "$AB" >/dev/null
expect 200 POST /api/auth/login/2fa "{\"ticket\":\"$(jq -r .ticket "$AB")\",\"code\":\"$N2\"}" "" 198.18.43.2
VERA=$(jq -r .token "$AB")

astep "two-factor: turning it off needs the password and a code"
expect 200 POST /api/auth/2fa/recovery-codes "{\"code\":\"$N3\"}" "$VERA"
OFF1=$(jq -r '.recovery_codes[0]' "$AB")
expect 400 POST /api/auth/2fa/disable "{\"password\":\"wrong\",\"code\":\"$OFF1\"}" "$VERA"
M11=$(mail_n)
expect 200 POST /api/auth/2fa/disable "{\"password\":\"new-password-4\",\"code\":\"$OFF1\"}" "$VERA"
wait_mail "$VERA_MAIL" "Two-factor sign-in was turned off" "$M11" >/dev/null
expect 200 POST /api/auth/login "{\"username\":\"vera$RUN\",\"password\":\"new-password-4\"}" "" 198.18.43.3
jq -e '(.token | length) == 64' "$AB" >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM recovery_codes WHERE user_id = $(uid_of "vera$RUN")")" = 0 ]
for a in 2fa_enabled 2fa_disabled 2fa_recovery_codes; do
  [ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = '$a' AND target_user_id = $(uid_of "vera$RUN")")" -ge 1 ]
done

astep "two-factor limits: 5 wrong codes per sign-in, 10 per account in 15 minutes"
LIM=$(register "limits$RUN" "limits$RUN@example.test")
expect 200 POST /api/auth/2fa/setup '{"password":"password123"}' "$LIM"
LSECRET=$(jq -r .secret "$AB")
expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$(totp "$LSECRET" -1)\"}" "$LIM"
expect 200 POST /api/auth/login "{\"username\":\"limits$RUN\",\"password\":\"password123\"}" "" 198.18.44.1
TICKET=$(jq -r .ticket "$AB")
for i in 1 2 3 4 5; do expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"00000$i\"}" "" 198.18.44.1; done
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$(totp "$LSECRET" 0)\"}" "" 198.18.44.1
grep -q "Sign in again" "$AB"
expect 200 POST /api/auth/login "{\"username\":\"limits$RUN\",\"password\":\"password123\"}" "" 198.18.44.1
TICKET=$(jq -r .ticket "$AB")
for i in 1 2 3 4 5; do expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"11111$i\"}" "" 198.18.44.1; done
# The 11th code of the window is refused even when it's right.
expect 429 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$(totp "$LSECRET" 0)\"}" "" 198.18.44.1
# Expired tickets (5 minutes).
psql "$DATABASE_URL" -qc "UPDATE pending_logins SET expires_at = now() - interval '1 second' WHERE user_id = $(uid_of "limits$RUN")"
expect 400 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"123456\"}" "" 198.18.44.1
grep -q "took too long" "$AB"

astep "site owner: remove someone's two-factor (owner only, reason, logged, they're told)"
GUS=$(register "gus$RUN" "gus$RUN@example.test")
GUS_ID=$(uid_of "gus$RUN")
M12=$(mail_n)
expect 200 POST /api/users/@me/email/resend "" "$GUS"
expect 200 POST /api/auth/verify-email "{\"token\":\"$(link_token "$(wait_mail "gus$RUN@example.test" "Confirm your email" "$M12")")\"}"
expect 200 POST /api/auth/2fa/setup '{"password":"password123"}' "$GUS"
GSECRET=$(jq -r .secret "$AB")
expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$(totp "$GSECRET" -1)\"}" "$GUS"
expect 403 POST "/api/safety/users/$GUS_ID/remove-2fa" '{"reason":"lost phone"}' "$LIM"
expect 400 POST "/api/safety/users/$GUS_ID/remove-2fa" '{"reason":""}' "$OWNER_TOKEN"
expect 200 GET "/api/safety/users/$GUS_ID" "" "$OWNER_TOKEN"
jq -e '.sign_in.enabled == true and .sign_in.recovery_codes_left == 10 and .sign_in.email_verified == true and (.sign_in.email | contains("•••@example.test"))' "$AB" >/dev/null
M13=$(mail_n)
expect 200 POST "/api/safety/users/$GUS_ID/remove-2fa" '{"reason":"Lost phone and codes; checked in person"}' "$OWNER_TOKEN"
wait_mail "gus$RUN@example.test" "Two-factor sign-in was removed" "$M13" >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = '2fa_reset' AND target_user_id = $GUS_ID AND details LIKE '%Lost phone%'")" = 1 ]
expect 200 POST /api/auth/login "{\"username\":\"gus$RUN\",\"password\":\"password123\"}" "" 198.18.45.1
jq -e '(.token | length) == 64' "$AB" >/dev/null

astep "site owner: a one-time reset link for someone who lost their email (owner only, logged)"
expect 403 POST "/api/safety/users/$GUS_ID/reset-link" "" "$LIM"
expect 200 POST "/api/safety/users/$GUS_ID/reset-link" "" "$OWNER_TOKEN"
OPATH=$(jq -r .path "$AB")
[[ "$OPATH" == /reset\?token=* ]]
OT=${OPATH#/reset?token=}
expect 200 POST /api/auth/reset/check "{\"token\":\"$OT\"}"
[ "$(psql "$DATABASE_URL" -tAc "SELECT expires_at > now() + interval '23 hours' FROM email_tokens WHERE token_hash = encode(sha256('$OT'::bytea), 'hex')")" = t ]
expect 200 POST /api/auth/reset "{\"token\":\"$OT\",\"password\":\"owner-made-1\"}"
expect 400 POST /api/auth/reset "{\"token\":\"$OT\",\"password\":\"owner-made-2\"}"
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = 'reset_link' AND target_user_id = $GUS_ID")" = 1 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM safety_log WHERE action = 'password_reset' AND target_user_id = $GUS_ID AND details LIKE '%site owner made%'")" = 1 ]
expect 200 POST /api/auth/login "{\"username\":\"gus$RUN\",\"password\":\"owner-made-1\"}" "" 198.18.45.2

astep "site owner: Server status shows email, and the test email button"
expect 200 GET /api/safety/server-status "" "$OWNER_TOKEN"
jq -e '.email.configured == true and .email.sent >= 1 and .auth_key == "env"' "$AB" >/dev/null
expect 403 POST /api/safety/email-test '{"to":"x@example.test"}' "$LIM"
M14=$(mail_n)
expect 200 POST /api/safety/email-test "{\"to\":\"owner-test$RUN@example.test\"}" "$OWNER_TOKEN"
wait_mail "owner-test$RUN@example.test" "Outhouse test email" "$M14" >/dev/null
expect 200 GET /api/users/@me "" "$OWNER_TOKEN"
jq -e '.site_owner == true and (.two_factor_nudge == "strong" or .two_factor == true)' "$AB" >/dev/null

astep "people who manage an outhouse get the stronger two-factor nudge"
MOD=$(register "mod$RUN" "mod$RUN@example.test")
expect 200 POST /api/guilds '{"name":"Mod shed"}' "$MOD"
expect 200 GET /api/users/@me "" "$MOD"
jq -e '.two_factor_nudge == "strong"' "$AB" >/dev/null

astep "bots are untouched (no email, no two-factor)"
expect 200 GET /api/users/@me "" "$LIM"
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM users WHERE is_bot AND (email IS NOT NULL OR totp_secret IS NOT NULL)")" = 0 ]

astep "no SMTP settings: sign-up still needs an email, resets go through the owner, status says so"
PORT2=${AUTH_NOSMTP_PORT:-8183}
BIND_ADDR=127.0.0.1:$PORT2 SMTP_HOST='' MAIL_FROM='' "$SERVER_BIN" >/tmp/outhouse-nosmtp.log 2>&1 &
NOSMTP=$!
for _ in $(seq 1 60); do curl -fsS "http://127.0.0.1:$PORT2/api/health" >/dev/null 2>&1 && break; sleep 0.5; done
SAVE_BASE=$BASE; BASE=http://127.0.0.1:$PORT2
expect 200 GET /api/auth/registration
jq -e '.email_available == false' "$AB" >/dev/null
expect 400 POST /api/auth/register "{\"username\":\"nosmtp$RUN\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\"}" "" 198.18.46.1
NS=$(register "nosmtp$RUN" "nosmtp$RUN@example.test" 198.18.46.1)
expect 200 GET /api/users/@me "" "$NS"
jq -e '.email_available == false and .email_verified == false' "$AB" >/dev/null
expect 400 POST /api/users/@me/email/resend "" "$NS"
expect 200 POST /api/auth/forgot "{\"login\":\"vera$RUN\"}" "" 198.18.46.2
jq -e '.sent == false and .email_available == false and (.message | contains("server owner"))' "$AB" >/dev/null
expect 200 GET /api/safety/server-status "" "$OWNER_TOKEN"
jq -e '.email.configured == false and (.email.problem | contains("SMTP_HOST"))' "$AB" >/dev/null
expect 400 POST /api/safety/email-test '{}' "$OWNER_TOKEN"
kill $NOSMTP 2>/dev/null || true; wait $NOSMTP 2>/dev/null || true
BASE=$SAVE_BASE
rm -f "$JAR"

echo "Auth test passed."
