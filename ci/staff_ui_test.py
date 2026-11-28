"""Site staff and outhouse ownership transfers in headless Chrome (desktop
and phone): the owner's Staff page, the add-staff dialog (tiers and custom
permissions), a moderator's restricted STAFF section, the Approvals page,
Staff activity, the owner's "Transfer ownership…" dialog, the recipient's
banner and acceptance, the directory's staff-assisted transfer (both
routes), "I'm still here", and the email confirmation page. Fails on any
JavaScript error and saves screenshots (50xx-*.png).

Needs the server started with the SMTP sink (ci/smtp_sink.py, MAIL_DIR),
AUTH_SECRET_KEY, PUBLIC_URL and DATABASE_URL (for psql), and a site owner:
OWNER_USER (default carolui, made by ci/ui_test.py) signs in with
password123, or is registered when this runs on a new database.
"""

import base64
import glob
import hashlib
import hmac
import json
import os
import re
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
MAIL_DIR = os.environ.get("MAIL_DIR", "/tmp/outhouse-mail")
DB = os.environ["DATABASE_URL"]
OWNER_USER = os.environ.get("OWNER_USER", "carolui")
os.makedirs(SHOTS, exist_ok=True)
errors = []
suffix = str(int(time.time()))[-5:]
DESKTOP = {"viewport": {"width": 1280, "height": 800}}
PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
ip_n = [0]


def api(method, path, body=None, token=None, xff=None):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    if xff:
        req.add_header("X-Forwarded-For", xff)
    data = json.dumps(body).encode() if body is not None else None
    try:
        with urllib.request.urlopen(req, data) as res:
            text = res.read().decode()
            return json.loads(text) if text else None
    except urllib.error.HTTPError as e:
        sys.exit(f"{method} {path} -> {e.code}: {e.read().decode()}")


def next_ip():
    ip_n[0] += 1
    return f"198.18.70.{ip_n[0]}"


def psql(sql):
    return subprocess.run(["psql", DB, "-tAc", sql], check=True, capture_output=True, text=True).stdout.strip()


def totp(secret, offset=0):
    key = base64.b32decode(secret.replace(" ", "").upper() + "=" * (-len(secret.replace(" ", "")) % 8))
    if time.time() % 30 > 27.5:
        time.sleep(30 - time.time() % 30 + 0.2)
    step = int(time.time() // 30) + offset
    h = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
    o = h[-1] & 0x0F
    return f"{(struct.unpack('>I', h[o:o + 4])[0] & 0x7FFFFFFF) % 1_000_000:06d}"


def wait_mail(to, subject, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        for f in sorted(glob.glob(os.path.join(MAIL_DIR, "*.json")), reverse=True):
            with open(f) as fh:
                m = json.load(fh)
            if to in m["to"] and subject in m["subject"]:
                return m
        time.sleep(0.25)
    sys.exit(f"no mail to {to} about {subject!r}")


def person(name, display, confirmed=True, two_factor=False, dob="1990-01-01"):
    """A new account: (token, id, secret)."""
    r = api("POST", "/api/auth/register", {
        "username": name, "email": f"{name}@example.test", "password": "password123",
        "birth_date": dob, "display_name": display,
    }, xff=next_ip())
    if confirmed:
        psql(f"UPDATE users SET email_verified_at = now() WHERE username = '{name}'")
    secret = None
    if two_factor:
        secret = api("POST", "/api/auth/2fa/setup", {"password": "password123"}, r["token"])["secret"]
        api("POST", "/api/auth/2fa/confirm", {"code": totp(secret, -1)}, r["token"])
    return r["token"], r["user"]["id"], secret


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


def shot(pg, name):
    pg.wait_for_timeout(300)
    pg.screenshot(path=f"{SHOTS}/{name}.png")


def owner_token():
    req = urllib.request.Request(BASE + "/api/auth/login", method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("X-Forwarded-For", next_ip())
    try:
        with urllib.request.urlopen(req, json.dumps({"username": OWNER_USER, "password": "password123"}).encode()) as res:
            return json.loads(res.read().decode())["token"]
    except urllib.error.HTTPError:
        return person(OWNER_USER, "Carol")[0]


OWNER = owner_token()
assert api("GET", "/api/users/@me/site-owner", None, OWNER)["site_owner"], f"{OWNER_USER} isn't the site owner"

# ---- people and things to look at ----
MOD, MOD_ID, _ = person(f"maya{suffix}", "Maya", two_factor=True)
SEN, SEN_ID, _ = person(f"sid{suffix}", "Sid", two_factor=True)
SEN2, SEN2_ID, _ = person(f"suze{suffix}", "Suze", two_factor=True)
SUP, SUP_ID, _ = person(f"pip{suffix}", "Pip", two_factor=True)
NOFA, _, _ = person(f"nofa{suffix}", "Nofa")
VIC, VIC_ID, _ = person(f"vince{suffix}", "Vince")
REP, REP_ID, _ = person(f"rita{suffix}", "Rita")
for name, tier, perms in ((f"maya{suffix}", "moderator", []), (f"sid{suffix}", "senior", []), (f"suze{suffix}", "senior", []),
                          (f"pip{suffix}", "custom", ["people", "account_help", "report_status", "warn"])):
    api("POST", "/api/safety/staff", {"username": name, "tier": tier, "perms": perms}, OWNER)
# Pip turns two-factor off later (powers paused): make them active first.
g = api("POST", "/api/guilds", {"name": f"Back Paddock {suffix}"}, OWNER)
inv = api("POST", f"/api/guilds/{g['id']}/invites", None, OWNER)
for t in (VIC, REP, MOD):
    api("POST", f"/api/invites/{inv['code']}", None, t)
ch = psql(f"SELECT id FROM channels WHERE guild_id = {g['id']} AND kind = 0 ORDER BY position LIMIT 1")
m1 = api("POST", f"/api/channels/{ch}/messages", {"content": "You're all hopeless at raiding"}, VIC)
api("POST", f"/api/channels/{ch}/messages", {"content": "Steady on, Vince"}, REP)
api("POST", "/api/reports", {"target_kind": "message", "target_id": m1["id"], "category": "harassment", "details": "Again"}, REP)
m2 = api("POST", f"/api/channels/{ch}/messages", {"content": "something awful"}, VIC)
api("POST", "/api/reports", {"target_kind": "message", "target_id": m2["id"], "category": "child_safety", "details": "Please look"}, REP)
api("POST", f"/api/safety/users/{VIC_ID}/actions", {"action": "warn", "text": "Keep it friendly, please."}, MOD)
api("POST", f"/api/safety/users/{VIC_ID}/actions", {"action": "suspend", "days": 3, "text": "Cool off"}, SEN)
# A request waiting for approval (two-person rule).
api("POST", f"/api/safety/users/{VIC_ID}/ip-blocks", {"make_permanent": True, "permanent_reason": "Repeat harassment", "reason": "Ban evasion"}, SEN)

# Ownership: Olive owns an outhouse with Ray in it; Ray is offered it.
OLI, OLI_ID, _ = person(f"olive{suffix}", "Olive")
RAY, RAY_ID, _ = person(f"ray{suffix}", "Ray")
go = api("POST", "/api/guilds", {"name": f"Olive's Shed {suffix}"}, OLI)
gi = api("POST", f"/api/guilds/{go['id']}/invites", None, OLI)
api("POST", f"/api/invites/{gi['code']}", None, RAY)
# Olaf has been away for months: Ada (an admin) asked for his outhouse.
OLAF, OLAF_ID, _ = person(f"olaf{suffix}", "Olaf")
ADA, ADA_ID, _ = person(f"ada{suffix}", "Ada")
gl = api("POST", "/api/guilds", {"name": f"Olaf's Long Drop {suffix}"}, OLAF)
li = api("POST", f"/api/guilds/{gl['id']}/invites", None, OLAF)
api("POST", f"/api/invites/{li['code']}", None, ADA)
role = api("POST", f"/api/guilds/{gl['id']}/roles", {"name": "Admin", "permissions": "1099511627776"}, OLAF)
api("PUT", f"/api/guilds/{gl['id']}/members/{ADA_ID}/roles/{role['id']}", None, OLAF)
psql(f"UPDATE users SET last_active_at = now() - interval '120 days' WHERE id = {OLAF_ID}")

with sync_playwright() as p:
    browser = p.chromium.launch(**({"executable_path": os.environ["CHROME"]} if os.environ.get("CHROME") else {}))

    def signed_in(label, kw, token):
        pg = browser.new_page(**kw)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)});")
        pg.goto(BASE + "/app")
        pg.wait_for_selector("#app:not([hidden])", timeout=15000)
        return pg

    def settings_tab(pg, tab, wait):
        pg.evaluate(f"openSafetyConsole({json.dumps(tab)})")
        pg.wait_for_selector(wait, timeout=15000)

    # ---- the owner's Staff page ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"staff-page-{label}", kw, OWNER)
        settings_tab(pg, "staff", f"dialog.settings .staff-row[data-staff='{MOD_ID}']")
        assert pg.is_visible("dialog.settings button[data-tab=staffact]")
        if label == "phone":
            pg.evaluate("document.querySelector('dialog.settings .staff-list')?.scrollIntoView({block: 'start'})")
        shot(pg, f"5001-staff-page-{label}")
        if label == "desktop":
            pg.evaluate("document.querySelector('dialog.settings .sc-toggle-row')?.scrollIntoView({block: 'center'})")
            shot(pg, "5002-staff-settings")
            # Add staff: someone without two-factor is refused.
            pg.click("dialog.settings .staff-add-btn")
            pg.fill("dialog.staff-dialog .staff-username", f"nofa{suffix}")
            pg.wait_for_selector("dialog.staff-dialog .staff-cand-problem:has-text('two-factor')", timeout=10000)
            assert pg.is_disabled("dialog.staff-dialog button[type=submit]")
            shot(pg, "5003-add-staff-refused")
            # A good candidate, Custom permissions.
            pg.fill("dialog.staff-dialog .staff-username", f"rita{suffix}")
            pg.wait_for_selector("dialog.staff-dialog .staff-cand-problem", timeout=10000)
            psql(f"UPDATE users SET totp_secret = 'x' WHERE username = 'rita{suffix}'")
            pg.fill("dialog.staff-dialog .staff-username", f"rita{suffix} ")
            pg.wait_for_selector("dialog.staff-dialog .staff-cand-ok", timeout=10000)
            pg.click("dialog.staff-dialog .staff-tier-opt[data-tier=support]")
            pg.check("dialog.staff-dialog .staff-perm input[value=warn]")
            assert pg.get_attribute("dialog.staff-dialog .staff-tier-opt[data-tier=custom]", "aria-checked") == "true"
            shot(pg, "5004-add-staff-custom")
            pg.click("dialog.staff-dialog button[type=submit]")
            pg.wait_for_selector(".toast:has-text('is now staff')", timeout=10000)
            psql(f"UPDATE users SET totp_secret = NULL WHERE username = 'rita{suffix}'")
            api("DELETE", f"/api/safety/staff/{REP_ID}", None, OWNER)
        else:
            pg.click("dialog.settings .staff-add-btn")
            pg.fill("dialog.staff-dialog .staff-username", f"maya{suffix}")
            pg.wait_for_selector("dialog.staff-dialog .staff-cand-problem:has-text('already staff')", timeout=10000)
            shot(pg, "5005-add-staff-phone")
        pg.close()

    # ---- a moderator's restricted view ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"moderator-{label}", kw, MOD)
        pg.wait_for_selector("#safety-btn", timeout=15000)
        settings_tab(pg, "open", "dialog.settings .sc-row.sc-locked.child")
        heads = pg.eval_on_selector("dialog.settings", "d => d.textContent")
        assert "STAFF" in heads and "SITE OWNER" not in heads
        for owner_only in ("staff", "status", "log", "teens", "blocks", "police"):
            assert not pg.query_selector(f"dialog.settings button[data-tab={owner_only}]"), f"moderator sees {owner_only}"
        shot(pg, f"5006-moderator-reports-{label}")
        if label == "desktop":
            pg.click("dialog.settings .sc-row:not(.sc-locked):has-text('hopeless')")
            pg.wait_for_selector("dialog.settings .sc-subject", timeout=15000)
            assert not pg.query_selector("dialog.settings .sc-handover"), "moderators see police hand-over"
            assert not pg.query_selector("dialog.settings .sc-make-permanent")
            shot(pg, "5007-moderator-report-detail")
            settings_tab(pg, "people", "dialog.settings input[aria-label='Find someone']")
            pg.evaluate(f"openSafetyConsole('people', null, {json.dumps(VIC_ID)})")
            pg.wait_for_selector("dialog.settings [data-same-ip-for]", timeout=15000)
            assert not pg.query_selector("dialog.settings [data-show-ips]"), "moderators see IP addresses"
            shot(pg, "5008-moderator-people")
        pg.close()

    # ---- Approvals (two-person rule) ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"approvals-{label}", kw, SEN2)
        settings_tab(pg, "approvals", "dialog.settings .staff-req.st-pending .staff-approve")
        shot(pg, f"5009-approvals-{label}")
        pg.close()
    pg = signed_in("approvals-own", DESKTOP, SEN)
    settings_tab(pg, "approvals", "dialog.settings .staff-req.st-pending")
    assert not pg.query_selector("dialog.settings .staff-approve"), "the requester can approve their own request"
    shot(pg, "5010-approvals-requester")
    pg.close()

    # ---- Staff activity (owner) ----
    pg = signed_in("staff-activity", DESKTOP, OWNER)
    settings_tab(pg, "staffact", "dialog.settings .staff-act-row")
    shot(pg, "5011-staff-activity")
    pg.close()

    # ---- paused powers (two-factor off) ----
    psql(f"UPDATE users SET totp_secret = NULL WHERE id = {SUP_ID}")
    pg = signed_in("paused", PHONE, SUP)
    # The app loads staff status just after it shows: wait for it before
    # opening the page that depends on it.
    pg.wait_for_function("state.safety && state.safety.staff && state.safety.staff.paused_reason", timeout=15000)
    pg.evaluate("openUserSettings('staffnote')")
    pg.wait_for_selector("dialog.settings .staff-note:has-text('paused')", timeout=15000)
    shot(pg, "5012-staff-paused-phone")
    pg.close()

    # ---- the owner hands an outhouse over ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"transfer-{label}", kw, OLI)
        pg.evaluate(f"selectGuild({json.dumps(go['id'])})")
        pg.wait_for_function(f"state.members && state.members.some((m) => m.id === {json.dumps(RAY_ID)})", timeout=15000)
        if label == "desktop":
            pg.click(f".member[data-id='{RAY_ID}']", button="right")
            pg.click(".menu :text('Transfer ownership')")
        else:
            pg.evaluate(f"openTransferDialog(memberById({json.dumps(RAY_ID)}))")
        pg.wait_for_selector("dialog.xfer-dialog[open] .xfer-confirm", timeout=10000)
        pg.fill("dialog.xfer-dialog .xfer-confirm", f"ray{suffix}")
        pg.check("dialog.xfer-dialog .xfer-keep")
        pg.fill("dialog.xfer-dialog .xfer-password", "password123")
        shot(pg, f"5013-transfer-dialog-{label}")
        if label == "desktop":
            pg.click("dialog.xfer-dialog button[type=submit]")
            pg.wait_for_selector(".xfer-banner.xfer-outgoing", timeout=10000)
            shot(pg, "5014-transfer-waiting-owner")
        pg.close()
    wait_mail(f"ray{suffix}@example.test", "wants you to own")

    # ---- the recipient: banner and acceptance ----
    for label, kw in (("phone", PHONE), ("desktop", DESKTOP)):
        pg = signed_in(f"accept-{label}", kw, RAY)
        pg.wait_for_selector(".xfer-banner.xfer-incoming", timeout=15000)
        shot(pg, f"5015-transfer-offer-banner-{label}")
        pg.click(".xfer-banner .xfer-review")
        pg.wait_for_selector("dialog.xfer-dialog[open] .xfer-decline", timeout=10000)
        shot(pg, f"5016-transfer-accept-dialog-{label}")
        if label == "desktop":
            pg.click("dialog.xfer-dialog button[type=submit]")
            pg.wait_for_selector(".toast:has-text('You now own')", timeout=10000)
            pg.wait_for_selector(".xfer-banner", state="detached", timeout=10000)
        pg.close()
    assert psql(f"SELECT owner_id FROM guilds WHERE id = {go['id']}") == RAY_ID

    # ---- staff-assisted transfer from the Outhouses directory ----
    for label, kw in (("phone", PHONE), ("desktop", DESKTOP)):
        pg = signed_in(f"dir-transfer-{label}", kw, SEN)
        pg.evaluate(f"openSafetyConsole('outhouses', null, null, {{ outhouseId: {json.dumps(gl['id'])} }})")
        pg.wait_for_selector("dialog.settings .dir-transfer-btn", timeout=15000)
        pg.click("dialog.settings .dir-transfer-btn")
        pg.wait_for_selector("dialog.xfer-staff-dialog[open] .xfer-route[data-route=inactive].active", timeout=10000)
        pg.fill("dialog.xfer-staff-dialog .xfer-new-owner", f"ada{suffix}")
        pg.fill("dialog.xfer-staff-dialog .xfer-reason", "Ada asked in #help: Olaf left the game last year")
        shot(pg, f"5017-directory-transfer-{label}")
        if label == "desktop":
            pg.click("dialog.xfer-staff-dialog .xfer-route[data-route=verified]")
            assert "email" in pg.inner_text("dialog.xfer-staff-dialog button[type=submit]")
            shot(pg, "5018-directory-transfer-email-route")
            pg.click("dialog.xfer-staff-dialog .xfer-route[data-route=inactive]")
            pg.click("dialog.xfer-staff-dialog button[type=submit]")
            pg.wait_for_selector("dialog.settings .xfer-panel", timeout=10000)
            shot(pg, "5019-directory-transfer-under-way")
        pg.close()
    wait_mail(f"olaf{suffix}@example.test", "still yours")

    # ---- the owner: "I'm still here" ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"still-here-{label}", kw, OLAF)
        pg.wait_for_selector(".xfer-banner.xfer-objection .xfer-still-here", timeout=15000)
        shot(pg, f"5020-still-here-banner-{label}")
        if label == "phone":
            pg.click(".xfer-banner.xfer-objection button:has-text('Details')")
            pg.wait_for_selector("dialog.xfer-dialog[open]", timeout=10000)
            shot(pg, "5021-still-here-dialog-phone")
            pg.click("dialog.xfer-dialog button[type=submit]")
            pg.wait_for_selector(".toast:has-text('stays yours')", timeout=10000)
        pg.close()

    # ---- the email check: the owner's confirmation page ----
    VERN, VERN_ID, _ = person(f"vern{suffix}", "Vern")
    VAL, VAL_ID, _ = person(f"val{suffix}", "Val")
    gv = api("POST", "/api/guilds", {"name": f"Vern's Dunny {suffix}"}, VERN)
    vi = api("POST", f"/api/guilds/{gv['id']}/invites", None, VERN)
    api("POST", f"/api/invites/{vi['code']}", None, VAL)
    api("POST", f"/api/safety/outhouses/{gv['id']}/transfer", {"route": "verified", "username": f"val{suffix}", "reason": "Vern lost his phone and password"}, OWNER)
    m = wait_mail(f"vern{suffix}@example.test", "Confirm the transfer")
    token = re.search(r"token=([0-9a-f]{64})", m["text"]).group(1)
    for label, kw in (("phone", PHONE), ("desktop", DESKTOP)):
        pg = browser.new_page(**kw)
        watch(pg, f"confirm-transfer-{label}")
        pg.goto(f"{BASE}/confirm-transfer?token={token}")
        pg.wait_for_selector("h2:has-text('Confirm the transfer')", timeout=10000)
        assert "token" not in pg.url
        shot(pg, f"5022-confirm-transfer-page-{label}")
        if label == "desktop":
            pg.click("button:has-text('Yes, transfer it')")
            pg.wait_for_selector("h2:has-text('Transfer confirmed')", timeout=10000)
            shot(pg, "5023-confirm-transfer-done")
        pg.close()

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("staff UI test passed")
