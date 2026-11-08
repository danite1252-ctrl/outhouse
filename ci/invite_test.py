"""Invite links in headless Chrome: the Invite people dialog (copy link, expiry
and use limits, revoke), Outhouse settings → Invites, the /invite/<code> page
signed out, signed in and inside the apps, the dead-invite page, and signing
up (or in) from an invite and landing straight in the outhouse.

Fails on any JavaScript error. Screenshots go to $SHOTS (printed by CI).
"""

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
os.makedirs(SHOTS, exist_ok=True)
errors = []

WINDOWS_UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) "
              "Chrome/126.0.0.0 Safari/537.36")
ANDROID_UA = ("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) "
              "Chrome/126.0.0.0 Mobile Safari/537.36")
DESKTOP = {"viewport": {"width": 1280, "height": 800}}
PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
ANDROID = {"viewport": {"width": 412, "height": 915}, "is_mobile": True, "has_touch": True, "user_agent": ANDROID_UA}
ANDROID_APP = {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}
IN_ANDROID_APP = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';"
IN_DESKTOP_APP = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'desktop'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';"


def api(method, path, body=None, token=None):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    data = json.dumps(body).encode() if body is not None else None
    with urllib.request.urlopen(req, data) as res:
        text = res.read().decode()
        return json.loads(text) if text else None


def register(name, display):
    return api("POST", "/api/auth/register", {"username": name, "password": "password123", "display_name": display})


suffix = str(int(time.time()))[-6:]
owner = register(f"gus{suffix}", "Gus")
mate = register(f"hazel{suffix}", "Hazel")       # signed in, not a member yet
member = register(f"ivy{suffix}", "Ivy")          # already a member
OWNER = owner["token"]
guild = api("POST", "/api/guilds", {"name": "The Long Drop"}, OWNER)
G = guild["id"]
code = api("POST", f"/api/guilds/{G}/invites", {"max_age_secs": 0, "max_uses": 0}, OWNER)["code"]
api("POST", f"/api/invites/{code}", None, member["token"])
dead = api("POST", f"/api/guilds/{G}/invites", None, OWNER)["code"]
api("DELETE", f"/api/invites/{dead}", None, OWNER)


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    # Chrome logs "Failed to launch 'outhouse://…'" when no app handles the link: that's the point of the test.
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and "Failed to launch" not in m.text
            and errors.append(f"[{label}] console: {m.text}"))


def shot(page, name):
    page.screenshot(path=f"{SHOTS}/{name}.png")


def page_for(browser, label, kw, token=None, init=""):
    ctx = browser.new_context(**kw)
    script = init
    if token:
        script += f"localStorage.setItem('outhouse_token', {json.dumps(token)});"
    if script:
        ctx.add_init_script(script)
    pg = ctx.new_page()
    watch(pg, label)
    return ctx, pg


def wait_in_app(pg, name):
    pg.wait_for_selector("#app:not([hidden])", timeout=15000)
    pg.wait_for_function(f"document.getElementById('guild-name').textContent === {json.dumps(name)}", timeout=15000)


with sync_playwright() as p:
    browser = p.chromium.launch()

    # ---- the Invite people dialog ----
    ctx, pg = page_for(browser, "invite-dialog", dict(DESKTOP, permissions=["clipboard-read", "clipboard-write"]), OWNER,
                       f"localStorage.setItem('outhouse_guild', {json.dumps(G)});")
    pg.goto(BASE + "/app")
    wait_in_app(pg, "The Long Drop")
    pg.click("#invite-btn")
    pg.wait_for_selector("dialog.invite-dialog")
    pg.wait_for_function("document.querySelector('.invite-dialog .inv-link').value.includes('/invite/')", timeout=10000)
    link = pg.input_value(".invite-dialog .inv-link")
    assert re.fullmatch(re.escape(BASE) + r"/invite/[A-Za-z0-9]{8}", link), link
    assert "Expires in 7 days" in pg.inner_text(".invite-dialog .inv-meta"), pg.inner_text(".invite-dialog .inv-meta")
    pg.click(".invite-dialog .inv-copy")
    pg.wait_for_selector(".invite-dialog .inv-copy:has-text('Copied!')")
    assert pg.evaluate("navigator.clipboard.readText()") == link
    shot(pg, "300-invite-dialog")
    # Opening it again reuses the same link instead of making another.
    pg.keyboard.press("Escape")
    pg.wait_for_selector("dialog.invite-dialog", state="detached")
    pg.click("#invite-btn")
    pg.wait_for_function("document.querySelector('.invite-dialog .inv-link').value.includes('/invite/')", timeout=10000)
    assert pg.input_value(".invite-dialog .inv-link") == link, "a second link was made"
    # 1 hour, 5 uses.
    pg.click(".invite-dialog .inv-options summary")
    pg.select_option(".invite-dialog .inv-option-grid select >> nth=0", "3600")
    pg.select_option(".invite-dialog .inv-option-grid select >> nth=1", "5")
    pg.click(".invite-dialog .inv-new")
    pg.wait_for_function(f"document.querySelector('.invite-dialog .inv-link').value !== {json.dumps(link)}", timeout=10000)
    meta = pg.inner_text(".invite-dialog .inv-meta")
    assert "Expires in 1 h" in meta and "5 uses" in meta, meta
    link2 = pg.input_value(".invite-dialog .inv-link")
    code2 = link2.rsplit("/", 1)[1]
    inv2 = next(i for i in api("GET", f"/api/guilds/{G}/invites", None, OWNER) if i["code"] == code2)
    assert inv2["max_uses"] == 5 and inv2["expires_at"], inv2
    pg.wait_for_selector(f".invite-dialog .inv-row[data-code='{code2}']")
    pg.wait_for_timeout(300)
    shot(pg, "301-invite-dialog-options")
    # Revoke the new one from the list: the link box says so.
    pg.click(f".invite-dialog .inv-row[data-code='{code2}'] .inv-revoke")
    pg.wait_for_selector(f".invite-dialog .inv-row[data-code='{code2}']", state="detached")
    assert "revoked" in pg.input_value(".invite-dialog .inv-link")
    try:
        api("GET", f"/api/invites/{code2}")
        raise AssertionError("revoked invite still works")
    except urllib.error.HTTPError as e:
        assert e.code == 404, e.code
    pg.keyboard.press("Escape")
    # Outhouse settings → Invites.
    pg.evaluate("openSettings('invites')")
    pg.wait_for_selector("dialog.settings .settings-nav button.active:has-text('Invites')")
    pg.wait_for_selector(f"dialog.settings .inv-row[data-code='{code}']", timeout=10000)
    shot(pg, "302-settings-invites")
    pg.keyboard.press("Escape")
    # The join box takes a whole link too.
    other = api("POST", "/api/guilds", {"name": "Back Shed"}, mate["token"])
    shed = api("POST", f"/api/guilds/{other['id']}/invites", None, mate["token"])
    pg.click("#join-guild")
    pg.fill("dialog input", shed["url"])
    pg.click("dialog button.primary")
    wait_in_app(pg, "Back Shed")
    ctx.close()

    # Phone and Android app widths of the dialog.
    for label, kw, init, n in (("phone", PHONE, "", 303), ("android-app", ANDROID_APP, IN_ANDROID_APP, 304)):
        ctx, pg = page_for(browser, f"invite-dialog-{label}", kw, OWNER, init + f"localStorage.setItem('outhouse_guild', {json.dumps(G)});")
        pg.goto(BASE + "/app")
        wait_in_app(pg, "The Long Drop")
        pg.wait_for_timeout(500)
        pg.evaluate("invitePeople()")
        pg.wait_for_function("document.querySelector('.invite-dialog .inv-link')?.value.includes('/invite/')", timeout=10000)
        pg.click(".invite-dialog .inv-options summary")
        pg.wait_for_timeout(300)
        shot(pg, f"{n}-invite-dialog-{label}")
        ctx.close()

    # ---- the /invite/<code> page, signed out ----
    for label, kw, n in (("desktop", dict(DESKTOP, user_agent=WINDOWS_UA), 310), ("phone", PHONE, 311), ("android", ANDROID, 312)):
        ctx, pg = page_for(browser, f"invite-page-{label}", kw)
        pg.goto(f"{BASE}/invite/{code}")
        pg.wait_for_selector(".inv-card.only-valid")
        assert pg.inner_text("#inv-title") == "The Long Drop"
        assert "Gus" in pg.inner_text(".inv-from")
        assert "2 members" in pg.inner_text(".inv-stats"), pg.inner_text(".inv-stats")
        assert pg.inner_text("#inv-primary") == "Accept invite"
        assert pg.get_attribute("#inv-primary", "href") == f"/app?register&invite={code}"
        assert pg.is_visible("#inv-signin")
        assert pg.get_attribute('meta[property="og:title"]', "content") == "You've been invited to The Long Drop on Outhouse"
        assert pg.is_visible("#inv-download a[href='/#download']")
        if label == "desktop":
            assert pg.is_visible("#inv-open-app") and pg.get_attribute("#inv-open-app", "href") == f"outhouse://invite/{code}"
        elif label == "android":
            href = pg.get_attribute("#inv-open-app", "href")
            assert pg.is_visible("#inv-open-app") and href.startswith(f"intent://127.0.0.1:8080/invite/{code}#Intent;scheme=http;package=cc.hartspace.outhouse;"), href
            assert "S.browser_fallback_url=" in href
        else:
            assert not pg.is_visible("#inv-open-app"), "open-in-app showed on an iPhone-ish phone"
        pg.wait_for_timeout(700)
        shot(pg, f"{n}-invite-page-{label}")
        if label == "desktop":
            # No app installed here: after a moment, the "Download it" nudge.
            pg.click("#inv-open-app")
            pg.wait_for_selector("#inv-noapp:not([hidden])", timeout=5000)
            assert pg.url.startswith(f"{BASE}/invite/"), pg.url
            shot(pg, "313-invite-page-no-app")
            pg.click("#inv-continue")
            assert pg.is_hidden("#inv-noapp")
        ctx.close()

    # ---- dead invites ----
    for label, kw, n in (("desktop", DESKTOP, 314), ("phone", PHONE, 315)):
        ctx, pg = page_for(browser, f"dead-invite-{label}", kw)
        res = pg.goto(f"{BASE}/invite/{dead}")
        assert res.status == 404, res.status
        pg.wait_for_selector(".inv-dead")
        assert "gone down the long drop" in pg.inner_text("#inv-dead-title")
        assert pg.get_attribute("#inv-dead-home", "href") == "/"
        assert not pg.is_visible("#inv-primary")
        pg.wait_for_timeout(500)
        shot(pg, f"{n}-invite-dead-{label}")
        ctx.close()

    # ---- signed in, not a member yet: Join → straight into the outhouse ----
    for label, kw, n in (("desktop", DESKTOP, 316), ("phone", PHONE, 317)):
        tok = mate["token"] if label == "desktop" else register(f"jack{suffix}", "Jack")["token"]
        ctx, pg = page_for(browser, f"invite-join-{label}", kw, tok)
        pg.goto(f"{BASE}/invite/{code}")
        pg.wait_for_selector("#inv-primary:has-text('Join The Long Drop')")
        pg.wait_for_selector("#inv-who:not([hidden])")
        assert "Signed in as" in pg.inner_text("#inv-who")
        assert not pg.is_visible("#inv-signin")
        pg.wait_for_timeout(500)
        shot(pg, f"{n}-invite-page-signed-in-{label}")
        pg.click("#inv-primary")
        wait_in_app(pg, "The Long Drop")
        assert pg.evaluate("location.pathname + location.search") == "/app", pg.url
        ctx.close()

    # ---- already a member: Open ----
    ctx, pg = page_for(browser, "invite-member", DESKTOP, member["token"])
    pg.goto(f"{BASE}/invite/{code}")
    pg.wait_for_selector("#inv-primary:has-text('Open The Long Drop')")
    assert "already in here" in pg.inner_text("#inv-who")
    assert pg.get_attribute("#inv-primary", "href") == f"/app?guild={G}"
    shot(pg, "318-invite-page-member")
    pg.click("#inv-primary")
    wait_in_app(pg, "The Long Drop")
    ctx.close()

    # ---- signed out: Accept → sign up → joined and in the outhouse ----
    uses_before = next(i for i in api("GET", f"/api/guilds/{G}/invites", None, OWNER) if i["code"] == code)["uses"]
    for label, kw, n in (("desktop", DESKTOP, 320), ("phone", PHONE, 322)):
        ctx, pg = page_for(browser, f"invite-register-{label}", kw)
        pg.goto(f"{BASE}/invite/{code}")
        pg.click("#inv-primary")
        pg.wait_for_selector("#display-name-field:not([hidden])")
        pg.wait_for_selector(".auth-invite:has-text('The Long Drop')", timeout=10000)
        assert "Gus invited you to" in pg.inner_text(".auth-invite")
        assert pg.evaluate("location.pathname + location.search") == "/app", pg.url
        assert pg.inner_text("#auth-submit") == "Create account"
        pg.wait_for_timeout(300)
        shot(pg, f"{n}-invite-register-{label}")
        pg.fill("input[name=username]", f"newbie{label[:2]}{suffix}")
        pg.fill("input[name=display_name]", f"Newbie {label}")
        pg.fill("input[name=password]", "password123")
        pg.click("#auth-submit")
        wait_in_app(pg, "The Long Drop")
        pg.wait_for_selector(".toast:has-text('Welcome to The Long Drop')", timeout=8000)
        pg.wait_for_timeout(500)
        shot(pg, f"{n + 1}-invite-register-joined-{label}")
        assert pg.evaluate("sessionStorage.getItem('outhouse_pending_invite')") is None
        ctx.close()
    uses_after = next(i for i in api("GET", f"/api/guilds/{G}/invites", None, OWNER) if i["code"] == code)["uses"]
    assert uses_after == uses_before + 2, (uses_before, uses_after)

    # ---- signed out: "Already have an account? Sign in" → sign in → joined ----
    kim = register(f"kim{suffix}", "Kim")
    ctx, pg = page_for(browser, "invite-login", DESKTOP)
    pg.goto(f"{BASE}/invite/{code}")
    pg.click("#inv-signin")
    pg.wait_for_selector("#auth:not([hidden])")
    assert pg.inner_text("#auth-submit") == "Sign in"
    pg.wait_for_selector(".auth-invite:has-text('The Long Drop')", timeout=10000)
    pg.fill("input[name=username]", f"kim{suffix}")
    pg.fill("input[name=password]", "password123")
    pg.click("#auth-submit")
    wait_in_app(pg, "The Long Drop")
    assert any(g["id"] == G for g in api("GET", "/api/guilds", None, kim["token"]))
    ctx.close()

    # A dead invite on the sign-in screen says so (and you can still sign in).
    ctx, pg = page_for(browser, "invite-login-dead", DESKTOP)
    pg.goto(f"{BASE}/app?invite={dead}")
    pg.wait_for_selector(".auth-invite.dead", timeout=10000)
    shot(pg, "324-invite-register-dead")
    ctx.close()

    # ---- inside the apps: the in-app join flow, no "open in app" / downloads ----
    for label, init, kw, n in (("android-app", IN_ANDROID_APP, ANDROID_APP, 325), ("desktop-app", IN_DESKTOP_APP, DESKTOP, 327)):
        ctx, pg = page_for(browser, f"invite-{label}", kw, None, init)
        pg.goto(f"{BASE}/invite/{code}")
        pg.wait_for_selector(".inv-card.only-valid")
        assert pg.evaluate("document.documentElement.classList.contains('in-app')")
        assert not pg.is_visible("#inv-open-app") and not pg.is_visible("#inv-download")
        assert pg.inner_text("#inv-primary") == "Accept invite"
        assert pg.get_attribute("#inv-home", "href") == "/app"
        pg.wait_for_timeout(500)
        shot(pg, f"{n}-invite-page-{label}")
        ctx.close()
    # Signed in inside the Android app: Join goes straight in.
    lou = register(f"lou{suffix}", "Lou")
    ctx, pg = page_for(browser, "invite-android-app-join", ANDROID_APP, lou["token"], IN_ANDROID_APP)
    pg.goto(f"{BASE}/invite/{code}")
    pg.wait_for_selector("#inv-primary:has-text('Join The Long Drop')")
    pg.wait_for_timeout(400)
    shot(pg, "326-invite-page-android-app-signed-in")
    pg.click("#inv-primary")
    wait_in_app(pg, "The Long Drop")
    ctx.close()
    # A dead invite inside the app goes back to the app, not the welcome page.
    ctx, pg = page_for(browser, "dead-invite-android-app", ANDROID_APP, None, IN_ANDROID_APP)
    pg.goto(f"{BASE}/invite/{dead}")
    pg.wait_for_selector(".inv-dead")
    assert pg.get_attribute("#inv-dead-home", "href") == "/app"
    shot(pg, "328-invite-dead-android-app")
    ctx.close()

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("Invite test passed.")
