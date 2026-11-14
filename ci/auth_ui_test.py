"""Email, password resets and two-factor sign-in in headless Chrome (desktop
and phone): sign-up with an email, the banners, "Forgot password?", the
reset page, setting up two-factor (QR code, recovery codes), the code step
with "Trust this device", trusted devices, and the site owner's tools.
Fails on any JavaScript error and saves screenshots (20xx-*.png).

Needs the server started with the SMTP sink (ci/smtp_sink.py, MAIL_DIR),
AUTH_SECRET_KEY and DATABASE_URL (for psql), and a site owner: OWNER_USER
(default carolui, made by ci/ui_test.py) signs in with password123, or is
registered when this runs on a new database.
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
suffix = str(int(time.time()))[-6:]
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
    with urllib.request.urlopen(req, data) as res:
        text = res.read().decode()
        return json.loads(text) if text else None


def next_ip():
    ip_n[0] += 1
    return f"198.18.60.{ip_n[0]}"


def register(name, email=None):
    return api("POST", "/api/auth/register", {
        "username": name, "email": email or f"{name}@example.test", "password": "password123",
        "birth_date": "1990-01-01", "display_name": name.capitalize().rstrip("0123456789"),
    }, xff=next_ip())


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


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


def shot(pg, name):
    pg.wait_for_timeout(250)
    pg.screenshot(path=f"{SHOTS}/{name}.png")


def owner_token():
    try:
        return api("POST", "/api/auth/login", {"username": OWNER_USER, "password": "password123"}, xff=next_ip())["token"]
    except urllib.error.HTTPError:
        return register(OWNER_USER)["token"]


OWNER = owner_token()
assert api("GET", "/api/users/@me", None, OWNER)["site_owner"], f"{OWNER_USER} isn't the site owner"
guild = api("POST", "/api/guilds", {"name": "Long Drop"}, OWNER)
invite = api("POST", f"/api/guilds/{guild['id']}/invites", None, OWNER)

with sync_playwright() as p:
    browser = p.chromium.launch(**({"executable_path": os.environ["CHROME"]} if os.environ.get("CHROME") else {}))

    def page(label, kw, token=None):
        pg = browser.new_page(**kw)
        watch(pg, label)
        if token:
            pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)});")
        return pg

    def signed_in(label, kw, token):
        pg = page(label, kw, token)
        pg.goto(BASE + "/app")
        pg.wait_for_selector("#app:not([hidden])", timeout=15000)
        return pg

    def join(token):
        api("POST", f"/api/invites/{invite['code']}", None, token)

    # ---- sign-up asks for an email (desktop and phone) ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = page(f"signup-{label}", kw)
        pg.route(BASE + "/api/auth/**", lambda route: route.continue_(headers={**route.request.headers, "x-forwarded-for": next_ip()}))
        pg.goto(BASE + "/app?register")
        pg.wait_for_selector("#email-field:not([hidden]) input[required]", timeout=10000)
        assert pg.is_hidden("#auth-forgot"), "Forgot password? shows on the sign-up form"
        pg.fill("input[name=username]", f"sign{label[:2]}{suffix}")
        pg.fill("input[name=display_name]", "Sam")
        pg.fill("input[name=email]", f"sam.{label}{suffix}@example.test")
        pg.fill("input[name=birth_date]", "1995-04-12")
        pg.fill("input[name=password]", "password123")
        pg.wait_for_selector("#email-hint:has-text('send a link to confirm')", timeout=10000)
        shot(pg, f"2001-signup-email-{label}")
        pg.click("#auth-submit")
        # Signed in straight away; the banner asks to confirm the email.
        pg.wait_for_selector(".acct-banner.acct-confirm-email:has-text('Confirm your email')", timeout=15000)
        assert f"sam.{label}{suffix}@example.test" in pg.inner_text(".acct-banner")
        shot(pg, f"2002-confirm-email-banner-{label}")
        # Resend works (and says where).
        pg.click(".acct-banner button:has-text('Resend')")
        pg.wait_for_selector(".toast:has-text('Sent another link')", timeout=10000)
        pg.close()
    m = wait_mail(f"sam.desktop{suffix}@example.test", "Confirm your email")
    token = re.search(r"token=([0-9a-f]{64})", m["text"]).group(1)

    # ---- the confirmation page ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = page(f"verify-{label}", kw)
        pg.goto(f"{BASE}/verify-email?token={token}")
        if label == "desktop":
            pg.wait_for_selector("h2:has-text('Email confirmed')", timeout=10000)
            assert "token" not in pg.url, "the token stayed in the address bar"
        else:
            pg.wait_for_selector("h2:has-text('This link isn')", timeout=10000)
        shot(pg, f"2003-verify-email-{'done' if label == 'desktop' else 'used-phone'}")
        pg.close()

    # ---- an account from before emails: the add-email warning ----
    old = register(f"oldtimer{suffix}")
    psql(f"UPDATE users SET email = NULL, email_verified_at = NULL WHERE username = 'oldtimer{suffix}'")
    for label, kw in (("phone", PHONE), ("desktop", DESKTOP)):
        pg = signed_in(f"addemail-{label}", kw, old["token"])
        pg.wait_for_selector(".acct-banner.acct-add-email", timeout=15000)
        shot(pg, f"2004-add-email-banner-{label}")
        if label == "desktop":
            pg.click(".acct-banner button:has-text('Add email')")
            pg.wait_for_selector("dialog.acct-dialog[open]")
            pg.fill("dialog.acct-dialog input[type=email]", f"oldtimer{suffix}@example.test")
            pg.fill("dialog.acct-dialog input[type=password]", "password123")
            shot(pg, "2005-add-email-dialog")
            pg.click("dialog.acct-dialog button[type=submit]")
            pg.wait_for_selector(".acct-banner.acct-confirm-email", timeout=10000)
            # Dismiss for a day.
            pg.click(".acct-banner-x")
            pg.wait_for_selector(".acct-banner.acct-confirm-email", state="detached")
        pg.close()

    # ---- forgot password (neutral answer) and the reset page ----
    rae = register(f"rae{suffix}")
    psql(f"UPDATE users SET email_verified_at = now() WHERE username = 'rae{suffix}'")
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = page(f"forgot-{label}", kw)
        pg.route(BASE + "/api/auth/**", lambda route: route.continue_(headers={**route.request.headers, "x-forwarded-for": next_ip()}))
        pg.goto(BASE + "/login")
        pg.wait_for_selector("#auth-form:not([hidden])")
        shot(pg, f"2006-sign-in-forgot-link-{label}")
        pg.fill("input[name=username]", f"rae{suffix}")
        pg.click("#auth-forgot")
        pg.wait_for_selector("#auth-forgot-form:not([hidden])")
        assert pg.input_value("#auth-forgot-login") == f"rae{suffix}"
        pg.click("#auth-forgot-submit")
        pg.wait_for_selector("#auth-forgot-done:not([hidden]):has-text('If that account has a confirmed email')", timeout=10000)
        shot(pg, f"2007-forgot-password-sent-{label}")
        pg.close()
    m = wait_mail(f"rae{suffix}@example.test", "Reset your Outhouse password")
    reset_token = re.search(r"token=([0-9a-f]{64})", m["text"]).group(1)
    pg = page("reset-desktop", DESKTOP)
    pg.goto(f"{BASE}/reset?token={reset_token}")
    pg.wait_for_selector("form.reset-form #password", timeout=10000)
    assert "token" not in pg.url
    pg.fill("#password", "brand-new-pass")
    pg.fill("#password2", "brand-new-pass")
    shot(pg, "2008-reset-page")
    pg.click("form.reset-form button[type=submit]")
    pg.wait_for_selector("h2:has-text('Password changed')", timeout=10000)
    shot(pg, "2009-reset-done")
    pg.close()
    api("POST", "/api/auth/login", {"username": f"rae{suffix}", "password": "brand-new-pass"}, xff=next_ip())

    # ---- the two-factor suggestion banner (gentle, and stronger for moderators) ----
    nia = register(f"nia{suffix}")
    psql(f"UPDATE users SET email_verified_at = now() WHERE username = 'nia{suffix}'")
    join(nia["token"])
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"nudge-{label}", kw, nia["token"])
        pg.wait_for_selector(".acct-banner.acct-2fa:not(.strong):has-text('Protect your account')", timeout=15000)
        shot(pg, f"2010-2fa-suggestion-{label}")
        pg.close()
    pg = signed_in("nudge-owner", DESKTOP, OWNER)
    psql(f"UPDATE users SET email_verified_at = now(), email = COALESCE(email, '{OWNER_USER}@example.test') WHERE username = '{OWNER_USER}'")
    pg.reload()
    pg.wait_for_selector(".acct-banner.acct-2fa.strong", timeout=15000)
    shot(pg, "2011-2fa-suggestion-strong-owner")
    # "Not now" snoozes it.
    pg.click(".acct-banner button:has-text('Not now')")
    pg.wait_for_selector(".acct-banner", state="detached")
    pg.close()

    # ---- setting up two-factor from the banner: QR code, then recovery codes ----
    secrets = {}
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        who = nia if label == "desktop" else register(f"pia{suffix}")
        if label == "phone":
            psql(f"UPDATE users SET email_verified_at = now() WHERE username = 'pia{suffix}'")
        pg = signed_in(f"setup-{label}", kw, who["token"])
        pg.wait_for_selector(".acct-banner.acct-2fa", timeout=15000)
        pg.click(".acct-banner button:has-text('Set up')")
        pg.wait_for_selector("dialog.acct-dialog[open] input[type=password]")
        pg.fill("dialog.acct-dialog input[type=password]", "password123")
        pg.click("dialog.acct-dialog button[type=submit]")
        pg.wait_for_selector("dialog.acct-dialog[open] img.acct-qr", timeout=10000)
        pg.wait_for_function("document.querySelector('img.acct-qr').naturalWidth > 0")
        secret = pg.inner_text("dialog.acct-dialog .acct-secret").replace(" ", "")
        secrets[label] = secret
        shot(pg, f"2012-2fa-setup-qr-{label}")
        pg.fill("dialog.acct-dialog .acct-code", "000000")
        pg.click("dialog.acct-dialog button[type=submit]")
        pg.wait_for_selector("dialog.acct-dialog .error:has-text('isn')", timeout=10000)
        pg.fill("dialog.acct-dialog .acct-code", totp(secret, -1))
        pg.click("dialog.acct-dialog button[type=submit]")
        pg.wait_for_selector("dialog.acct-dialog[open] ol.acct-codes li >> nth=9", timeout=10000)
        codes = pg.eval_on_selector_all("ol.acct-codes code", "els => els.map((e) => e.textContent)")
        assert len(codes) == 10 and all(re.fullmatch(r"[a-z0-9]{5}-[a-z0-9]{5}", c) for c in codes), codes
        if label == "desktop":
            nia_codes = codes
            with pg.expect_download() as dl:
                pg.click("dialog.acct-dialog button:has-text('Download .txt')")
            text = open(dl.value.path()).read()
            assert all(c in text for c in codes), "the download is missing codes"
        shot(pg, f"2013-2fa-recovery-codes-{label}")
        pg.click("dialog.acct-dialog button:has-text(\"I've saved them\")")
        pg.wait_for_selector("dialog.acct-dialog", state="detached")
        # No more suggestion.
        pg.wait_for_selector(".acct-banner.acct-2fa", state="detached", timeout=10000)
        pg.close()

    # ---- signing in with a code, trusting this device ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        user = f"nia{suffix}" if label == "desktop" else f"pia{suffix}"
        pg = page(f"code-{label}", kw)
        pg.route(BASE + "/api/auth/**", lambda route: route.continue_(headers={**route.request.headers, "x-forwarded-for": "198.18.61.1"}))
        pg.goto(BASE + "/login")
        pg.fill("input[name=username]", user)
        pg.fill("input[name=password]", "password123")
        pg.click("#auth-submit")
        pg.wait_for_selector("#auth-2fa-form:not([hidden])", timeout=10000)
        assert pg.is_checked("#auth-2fa-trust") and pg.input_value("#auth-2fa-days") == "30"
        shot(pg, f"2014-2fa-sign-in-step-{label}")
        if label == "desktop":
            pg.click("#auth-2fa-switch")
            pg.wait_for_selector("#auth-2fa-label:has-text('Recovery code')")
            shot(pg, "2015-2fa-sign-in-recovery-code")
            pg.click("#auth-2fa-switch")
        pg.fill("#auth-2fa-code", "123456")
        pg.click("#auth-2fa-submit")
        pg.wait_for_selector("#auth-2fa-error:has-text('isn')", timeout=10000)
        pg.fill("#auth-2fa-code", totp(secrets[label], 0))
        pg.click("#auth-2fa-submit")
        pg.wait_for_selector("#app:not([hidden])", timeout=15000)
        # Signed out and back in: this device is trusted, so no code step.
        pg.wait_for_selector("#messages .msg, #messages .empty, #messages > *", timeout=15000)
        pg.wait_for_timeout(1500)  # let the app finish loading before signing out
        pg.evaluate("signOut()")
        pg.wait_for_selector("#auth-form:not([hidden])", timeout=10000)
        pg.fill("input[name=username]", user)
        pg.fill("input[name=password]", "password123")
        pg.click("#auth-submit")
        pg.wait_for_selector("#app:not([hidden])", timeout=15000)
        assert pg.is_hidden("#auth-2fa-form")
        # Settings → My account: two-factor on, this device trusted.
        pg.evaluate("openUserSettings('account')")
        pg.wait_for_selector("dialog.settings .acct-2fa-section .acct-device", timeout=10000)
        pg.wait_for_selector("dialog.settings .acct-email-section")
        assert "This device" in pg.inner_text(".acct-device")
        pg.evaluate("document.querySelector('dialog.settings .acct-2fa-section').scrollIntoView({block: 'start'})")
        shot(pg, f"2016-2fa-settings-trusted-devices-{label}")
        if label == "desktop":
            pg.evaluate("document.querySelector('dialog.settings .acct-email-section').scrollIntoView({block: 'start'})")
            shot(pg, "2017-settings-email-section")
        pg.close()

    # ---- the reset page with two-factor on asks for a code (phone) ----
    link = api("POST", f"/api/safety/users/{nia['user']['id']}/reset-link", None, OWNER)
    pg = page("reset-2fa-phone", PHONE)
    pg.goto(BASE + link["path"])
    pg.wait_for_selector("form.reset-form #code", timeout=10000)
    pg.fill("#password", "another-new-pass")
    pg.fill("#password2", "another-new-pass")
    shot(pg, "2018-reset-page-2fa-phone")
    pg.fill("#code", "999999")
    pg.click("form.reset-form button[type=submit]")
    pg.wait_for_selector("form.reset-form .error:has-text('isn')", timeout=10000)
    pg.fill("#code", nia_codes[0])
    pg.click("form.reset-form button[type=submit]")
    pg.wait_for_selector("h2:has-text('Password changed')", timeout=10000)
    shot(pg, "2019-reset-done-phone")
    pg.close()

    # ---- site owner: People → Sign-in & recovery, and Server status → Email ----
    pg = signed_in("owner-people", DESKTOP, OWNER)
    pg.evaluate(f"openSafetyConsole('people', null, {json.dumps(nia['user']['id'])})")
    pg.wait_for_selector("dialog.settings .acct-owner-section .acct-remove-2fa-btn", timeout=15000)
    pg.evaluate("document.querySelector('dialog.settings .acct-owner-section').scrollIntoView({block: 'center'})")
    shot(pg, "2020-owner-people-sign-in")
    pg.click("dialog.settings .acct-reset-link-btn")
    pg.click("dialog:not(.settings) button:has-text('Make link')")
    pg.wait_for_selector("dialog.acct-dialog[open] .acct-link-field", timeout=10000)
    assert "/reset?token=" in pg.input_value("dialog.acct-dialog .acct-link-field")
    shot(pg, "2021-owner-reset-link")
    pg.click("dialog.acct-dialog button:has-text('Close')")
    pg.click("dialog.settings .acct-remove-2fa-btn")
    pg.fill("dialog.acct-dialog input[type=text]", "Lost phone and codes; checked with her on voice")
    shot(pg, "2022-owner-remove-2fa")
    pg.click("dialog.acct-dialog button[type=submit]")
    pg.wait_for_selector(".toast:has-text('Two-factor removed')", timeout=10000)
    pg.wait_for_selector("dialog.settings .acct-owner-section .acct-remove-2fa-btn", state="detached", timeout=10000)
    wait_mail(f"nia{suffix}@example.test", "Two-factor sign-in was removed")
    pg.evaluate("openSafetyConsole('status')")
    pg.wait_for_selector("dialog.settings .acct-status-block", timeout=15000)
    pg.fill(".acct-test-row input", f"owner{suffix}@example.test")
    pg.click(".acct-test-row button")
    pg.wait_for_selector(".acct-status-block [role=status]:has-text('Sent to')", timeout=20000)
    pg.evaluate("document.querySelector('dialog.settings .acct-status-block').scrollIntoView({block: 'center'})")
    shot(pg, "2023-server-status-email")
    pg.close()
    wait_mail(f"owner{suffix}@example.test", "Outhouse test email")

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("auth UI test passed")
