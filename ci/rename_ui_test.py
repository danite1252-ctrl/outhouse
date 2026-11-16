"""Forced username changes in headless Chrome (desktop and phone): a
moderator's "Force username change…" dialog in People, the person's
blocking "Choose a new username" dialog (live availability, then the app
opens), the site owner's Usernames page (blocked words) and People's
"Usernames matching blocked words" filter. Fails on any JavaScript error
and saves screenshots (97xx-*.png).

Needs the server with DATABASE_URL (for psql) and a site owner: OWNER_USER
(default carolui, made by ci/ui_test.py) signs in with password123, or is
registered when this runs on a new database.
"""

import base64
import hashlib
import hmac
import json
import os
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
DB = os.environ["DATABASE_URL"]
OWNER_USER = os.environ.get("OWNER_USER", "carolui")
os.makedirs(SHOTS, exist_ok=True)
errors = []
suffix = str(int(time.time()))[-5:]
DESKTOP = {"viewport": {"width": 1280, "height": 800}}
PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
ip_n = [0]


def api(method, path, body=None, token=None, ok=True):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    ip_n[0] += 1
    req.add_header("X-Forwarded-For", f"198.18.97.{ip_n[0] % 250 + 1}")
    data = json.dumps(body).encode() if body is not None else None
    try:
        with urllib.request.urlopen(req, data) as res:
            text = res.read().decode()
            return json.loads(text) if text else None
    except urllib.error.HTTPError as e:
        if not ok:
            return {"status": e.code, **json.loads(e.read().decode() or "{}")}
        sys.exit(f"{method} {path} -> {e.code}: {e.read().decode()}")


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


def person(name, display, two_factor=False):
    r = api("POST", "/api/auth/register", {
        "username": name, "email": f"{name}@example.test", "password": "password123",
        "birth_date": "1990-01-01", "display_name": display,
    })
    psql(f"UPDATE users SET email_verified_at = now() WHERE username = '{name}'")
    if two_factor:
        secret = api("POST", "/api/auth/2fa/setup", {"password": "password123"}, r["token"])["secret"]
        api("POST", "/api/auth/2fa/confirm", {"code": totp(secret, -1)}, r["token"])
    return r["token"], r["user"]["id"]


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


def shot(pg, name):
    pg.wait_for_timeout(300)
    pg.screenshot(path=f"{SHOTS}/{name}.png")


def owner_token():
    r = api("POST", "/api/auth/login", {"username": OWNER_USER, "password": "password123"}, ok=False)
    if r.get("token"):
        return r["token"]
    return person(OWNER_USER, "Carol")[0]


OWNER = owner_token()
assert api("GET", "/api/users/@me/site-owner", None, OWNER)["site_owner"], f"{OWNER_USER} isn't the site owner"

# A second run on the same database: start without this test's words.
for w in ("grotty", "stinky"):
    api("DELETE", "/api/safety/username-words", {"word": w}, OWNER)
MOD, MOD_ID = person(f"mona{suffix}", "Mona", two_factor=True)
api("POST", "/api/safety/staff", {"username": f"mona{suffix}", "tier": "moderator"}, OWNER)
RUDE, RUDE_ID = person(f"stinky.bum{suffix}", f"stinky.bum{suffix}")
PHONEY, PHONEY_ID = person(f"fake.admin{suffix}", "Fake Admin")
person(f"grotty{suffix}", "Grotty")      # matches the blocked word added below
person(f"pat{suffix}", "Pat")
g = api("POST", "/api/guilds", {"name": f"Long Drop {suffix}"}, OWNER)
inv = api("POST", f"/api/guilds/{g['id']}/invites", None, OWNER)
for t in (RUDE, PHONEY):
    api("POST", f"/api/invites/{inv['code']}", None, t)

with sync_playwright() as p:
    browser = p.chromium.launch(**({"executable_path": os.environ["CHROME"]} if os.environ.get("CHROME") else {}))

    def signed_in(label, kw, token, wait="#app:not([hidden])"):
        pg = browser.new_page(**kw)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)});")
        pg.goto(BASE + "/app")
        pg.wait_for_selector(wait, timeout=15000)
        return pg

    # ---- a moderator forces a change from People ----
    pg = signed_in("mod-people", DESKTOP, MOD)
    pg.wait_for_selector("#safety-btn", timeout=15000)
    pg.evaluate(f"openSafetyConsole('people', null, {json.dumps(RUDE_ID)})")
    pg.wait_for_selector("dialog.settings .sc-force-rename", timeout=15000)
    pg.click("dialog.settings .sc-force-rename")
    pg.wait_for_selector("dialog.rn-dialog[open] .rn-reason[data-reason=offensive]", timeout=10000)
    assert pg.is_checked("dialog.rn-dialog .rn-reset"), "display name same as username: reset should be ticked"
    pg.click("dialog.rn-dialog .rn-reason[data-reason=offensive]")
    pg.fill("dialog.rn-dialog .rn-text", "Rude word in the name")
    shot(pg, "9701-force-rename-dialog")
    pg.click("dialog.rn-dialog button[type=submit]")
    pg.wait_for_selector(".toast:has-text('choose a new one')", timeout=10000)
    pg.wait_for_selector("dialog.settings .rn-chip", timeout=10000)
    assert pg.is_disabled("dialog.settings .sc-force-rename")
    shot(pg, "9702-people-after-force")
    pg.close()
    placeholder = psql(f"SELECT username FROM users WHERE id = {RUDE_ID}")
    assert placeholder.startswith("renamed-"), placeholder

    # Phone: the same dialog from People.
    pg = signed_in("mod-people-phone", PHONE, MOD)
    pg.evaluate(f"openSafetyConsole('people', null, {json.dumps(PHONEY_ID)})")
    pg.wait_for_selector("dialog.settings .sc-force-rename", timeout=15000)
    pg.evaluate("document.querySelector('dialog.settings .sc-force-rename').click()")
    pg.wait_for_selector("dialog.rn-dialog[open]", timeout=10000)
    assert not pg.is_checked("dialog.rn-dialog .rn-reset")
    pg.click("dialog.rn-dialog .rn-reason[data-reason=impersonation]")
    shot(pg, "9703-force-rename-dialog-phone")
    pg.click("dialog.rn-dialog button[type=submit]")
    pg.wait_for_selector(".toast:has-text('choose a new one')", timeout=10000)
    pg.close()

    # ---- the owner blocks a word ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"owner-words-{label}", kw, OWNER)
        pg.evaluate("openSafetyConsole('names')")
        pg.wait_for_selector("dialog.settings .rn-word-input", timeout=15000)
        if label == "desktop":
            pg.fill("dialog.settings .rn-word-input", "Gr0tty")
            pg.click("dialog.settings .rn-add")
            pg.wait_for_selector("dialog.settings .rn-word[data-word=grotty]", timeout=10000)
            pg.fill("dialog.settings .rn-word-input", "stinky")
            pg.press("dialog.settings .rn-word-input", "Enter")
            pg.wait_for_selector("dialog.settings .rn-word[data-word=stinky]", timeout=10000)
            pg.wait_for_selector("dialog.settings .rn-review", timeout=10000)
        shot(pg, f"9704-blocked-words-{label}")
        if label == "desktop":
            pg.click("dialog.settings .rn-review")
            pg.wait_for_selector(f"dialog.settings .sc-row:has-text('grotty{suffix}')", timeout=10000)
            assert pg.eval_on_selector("dialog.settings .sc-people-filter", "s => s.value") == "blocked"
            assert not pg.query_selector(f"dialog.settings .sc-row:has-text('pat{suffix}')")
            shot(pg, "9705-people-blocked-filter")
        pg.close()

    # ---- the person: blocked until they choose (desktop, then phone) ----
    pg = signed_in("choose-desktop", DESKTOP, RUDE, wait="dialog.rn-choose[open]")
    assert pg.is_visible("#app"), "the app shell should be behind the dialog"
    assert "changed by Outhouse staff" in pg.inner_text("dialog.rn-choose")
    assert "Offensive" in pg.inner_text("dialog.rn-choose")
    pg.keyboard.press("Escape")
    assert pg.is_visible("dialog.rn-choose[open]"), "Escape closed the blocking dialog"
    pg.fill("dialog.rn-choose .rn-input", f"stinky.bum{suffix}")
    pg.wait_for_selector("dialog.rn-choose .rn-status.bad", timeout=10000)
    assert "isn't available" in pg.inner_text("dialog.rn-choose .rn-status")
    assert pg.is_disabled("dialog.rn-choose .rn-save")
    shot(pg, "9706-choose-username-taken")
    pg.fill("dialog.rn-choose .rn-input", f"fresh.start{suffix}")
    pg.wait_for_selector("dialog.rn-choose .rn-status.ok", timeout=10000)
    shot(pg, "9707-choose-username-desktop")
    pg.click("dialog.rn-choose .rn-save")
    pg.wait_for_selector("dialog.rn-choose", state="detached", timeout=10000)
    pg.wait_for_function(f"state.me && state.me.username === 'fresh.start{suffix}'", timeout=15000)
    pg.wait_for_selector("#conn-status.up", timeout=15000)
    shot(pg, "9708-after-choosing")
    pg.close()
    assert psql(f"SELECT must_choose_username FROM users WHERE id = {RUDE_ID}") == "f"

    pg = signed_in("choose-phone", PHONE, PHONEY, wait="dialog.rn-choose[open]")
    assert "Impersonation" in pg.inner_text("dialog.rn-choose")
    pg.fill("dialog.rn-choose .rn-input", f"gr0tty_pal{suffix}")
    pg.wait_for_selector("dialog.rn-choose .rn-status.bad", timeout=10000)
    assert "isn't allowed" in pg.inner_text("dialog.rn-choose .rn-status")
    pg.fill("dialog.rn-choose .rn-input", f"honest.abe{suffix}")
    pg.wait_for_selector("dialog.rn-choose .rn-status.ok", timeout=10000)
    shot(pg, "9709-choose-username-phone")
    pg.click("dialog.rn-choose .rn-signout")
    pg.wait_for_selector("#auth:not([hidden])", timeout=10000)
    pg.close()
    assert psql(f"SELECT must_choose_username FROM users WHERE id = {PHONEY_ID}") == "t"

    # A forced change while they're using the app: the dialog appears live.
    PAT = api("POST", "/api/auth/login", {"username": f"pat{suffix}", "password": "password123"})["token"]
    PAT_ID = psql(f"SELECT id FROM users WHERE username = 'pat{suffix}'")
    pg = signed_in("live", DESKTOP, PAT)
    pg.wait_for_function("state.ws && state.ws.readyState === 1", timeout=15000)
    pg.wait_for_timeout(500)
    api("POST", f"/api/safety/users/{PAT_ID}/actions", {"action": "force_rename", "reason": "inappropriate"}, OWNER)
    pg.wait_for_selector("dialog.rn-choose[open]", timeout=15000)
    pg.close()

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("rename UI test passed")
