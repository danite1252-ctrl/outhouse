"""Landing page at / and the routes around it, in headless Chrome.

- Logged-out visitors get the landing page (desktop, phone and Android widths).
- Signed-in people, notification deep links and the Windows/Android apps go
  straight to the web client at /app.
- Download buttons show versions when /updates has files, "Coming soon" when not.
Fails on any JavaScript error. Screenshots go to $SHOTS (printed by CI).
"""

import json
import os
import shutil
import sys
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
WEB_DIR = Path(os.environ.get("WEB_DIR", "web"))
NO_API = os.environ.get("LANDING_NO_API") == "1"  # local runs against a static server
os.makedirs(SHOTS, exist_ok=True)
errors = []


def get(path):
    with urllib.request.urlopen(BASE + path) as res:
        return res.status, res.headers, res.read()


def wait_for_server():
    for _ in range(60):
        try:
            urllib.request.urlopen(BASE + "/")
            return
        except Exception:
            time.sleep(1)
    sys.exit("server never came up")


wait_for_server()

# ---- plain HTTP: what's served where ----
status, headers, body = get("/")
assert status == 200 and b"Chat out" in body and b"landing.js" in body, "/ isn't the landing page"
assert "text/html" in headers.get("Content-Type", ""), headers
if not NO_API:
    assert headers.get("Cache-Control") == "no-cache", headers
for path in ("/app", "/login", "/index.html"):
    status, headers, body = get(path)
    assert status == 200 and b'id="auth"' in body and b"app.js" in body, f"{path} isn't the web client"
for path, ctype in (("/landing.css", "text/css"), ("/landing.js", "javascript"), ("/fonts/inter-400.woff", "woff"),
                    ("/manifest.webmanifest", "manifest"), ("/sw.js", "javascript")):
    status, headers, _ = get(path)
    assert status == 200 and ctype in headers.get("Content-Type", ""), (path, headers.get("Content-Type"))
manifest = json.loads(get("/manifest.webmanifest")[2])
assert manifest["start_url"] == "/app" and manifest["id"] == "/", manifest

token = None
if not NO_API:
    suffix = str(int(time.time()))[-6:]
    req = urllib.request.Request(BASE + "/api/auth/register", method="POST",
                                 data=json.dumps({"username": f"lander{suffix}", "password": "password123"}).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req) as res:
        token = json.loads(res.read())["token"]

# Pretend the server has mirrored app builds into /updates.
updates = WEB_DIR / "updates"
made_updates = not updates.exists()
updates.mkdir(exist_ok=True)
(updates / "latest.json").write_text(json.dumps({"version": "0.2.321", "platforms": {}}))
(updates / "android.json").write_text(json.dumps({"version": "0.2.321", "url": "/updates/Outhouse-Android.apk"}))
(updates / "Outhouse-Windows-Setup.exe").write_bytes(b"\0" * 4_200_000)
(updates / "Outhouse-Android.apk").write_bytes(b"\0" * 9_800_000)


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "Failed to load resource" not in m.text
            and errors.append(f"[{label}] console: {m.text}"))


def shot(page, name):
    page.screenshot(path=f"{SHOTS}/{name}.png")


def show(page, section, wait=1600):
    page.evaluate(f"document.getElementById({json.dumps(section)}).scrollIntoView({{ block: 'start' }})")
    page.wait_for_timeout(wait)


def path_of(page):
    return page.evaluate("location.pathname")


ANDROID_UA = ("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) "
              "Chrome/126.0.0.0 Mobile Safari/537.36")

try:
    with sync_playwright() as p:
        browser = p.chromium.launch()

        # ---- landing page, three sizes ----
        sizes = (
            ("desktop", {"viewport": {"width": 1440, "height": 900}}, 200),
            ("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 210),
            ("android", {"viewport": {"width": 412, "height": 915}, "is_mobile": True, "has_touch": True, "user_agent": ANDROID_UA}, 220),
        )
        for label, kw, n in sizes:
            ctx = browser.new_context(**kw)
            pg = ctx.new_page()
            watch(pg, f"landing-{label}")
            pg.goto(BASE + "/")
            pg.wait_for_selector("#stage.open")
            assert path_of(pg) == "/", path_of(pg)
            assert pg.is_visible("h1:has-text('Chat out')")
            assert pg.is_visible("a[data-signin]"), "no Sign in button"
            pg.wait_for_timeout(3600)  # door open, first chat bubbles out
            shot(pg, f"{n}-landing-{label}-hero")
            show(pg, "peek", 2200)
            shot(pg, f"{n + 1}-landing-{label}-peek")
            for i, sec in enumerate(("chat", "voice", "raids")):
                show(pg, sec, 2000)
                shot(pg, f"{n + 2 + i}-landing-{label}-{sec}")
            show(pg, "more")
            shot(pg, f"{n + 5}-landing-{label}-more")
            show(pg, "download")
            pg.wait_for_selector("[data-download='windows'][href$='Outhouse-Windows-Setup.exe']", timeout=10000)
            pg.wait_for_selector("[data-download='android'][href$='Outhouse-Android.apk']", timeout=10000)
            pg.wait_for_selector("[data-version='windows'] .ver:has-text('0.2.321')", timeout=10000)
            pg.wait_for_selector("[data-version='android'] .ver:has-text('0.2.321')", timeout=10000)
            assert "allow installs from this source" in pg.inner_text(".dl-card[data-platform='android']")
            if label == "android":
                assert pg.evaluate("document.querySelector('.dl-card[data-platform=\"android\"]').classList.contains('recommended')")
            shot(pg, f"{n + 6}-landing-{label}-download")
            pg.evaluate("scrollTo(0, document.body.scrollHeight)")
            pg.wait_for_timeout(1200)
            shot(pg, f"{n + 7}-landing-{label}-footer")
            # Keyboard: the skip link is the first stop and is visible when focused.
            pg.goto(BASE + "/")
            pg.wait_for_selector("#stage.open")
            pg.keyboard.press("Tab")
            assert pg.evaluate("document.activeElement.classList.contains('skip')"), "skip link isn't first"
            # Sign in goes to the web client's sign-in screen.
            pg.click("a[data-signin]")
            pg.wait_for_selector("#auth:not([hidden])")
            assert path_of(pg) == "/app", path_of(pg)
            if label == "desktop":
                shot(pg, f"{n + 8}-landing-signin")
                assert pg.is_visible("#auth-home"), "no way back to the landing page"
            ctx.close()

        # ---- downloads not there yet ----
        ctx = browser.new_context(viewport={"width": 1280, "height": 900})
        pg = ctx.new_page()
        watch(pg, "landing-no-downloads")
        pg.route("**/updates/**", lambda route: route.fulfill(status=404, body="not found"))
        pg.goto(BASE + "/#download")
        pg.wait_for_selector("[data-download='windows'].soon:has-text('Coming soon')", timeout=10000)
        pg.wait_for_selector("[data-download='android'].soon:has-text('Coming soon')", timeout=10000)
        assert pg.get_attribute("[data-download='windows']", "href") is None
        show(pg, "download")
        shot(pg, "230-landing-downloads-missing")
        ctx.close()

        # ---- less motion: everything visible, door already open ----
        ctx = browser.new_context(viewport={"width": 1280, "height": 800}, reduced_motion="reduce")
        pg = ctx.new_page()
        watch(pg, "landing-reduced-motion")
        pg.goto(BASE + "/")
        pg.wait_for_timeout(600)
        shot(pg, "231-landing-reduced-motion")
        show(pg, "more", 300)
        assert pg.evaluate("getComputedStyle(document.querySelector('.tile')).opacity") == "1"
        ctx.close()

        # ---- sign-up link: /app?register opens the sign-up form, tidy URL ----
        ctx = browser.new_context(viewport={"width": 1280, "height": 800})
        pg = ctx.new_page()
        watch(pg, "landing-register")
        pg.goto(BASE + "/")
        pg.click(".hero a[data-cta='register']")
        pg.wait_for_selector("#display-name-field:not([hidden])")
        assert pg.evaluate("location.pathname + location.search") == "/app", pg.url
        assert pg.inner_text("#auth-submit") == "Create account"
        shot(pg, "232-landing-register")
        # A notification link from a logged-out browser still lands in the web client.
        pg.goto(BASE + "/?guild=123&channel=456")
        pg.wait_for_selector("#auth:not([hidden])")
        assert path_of(pg) == "/app", pg.url
        ctx.close()

        # ---- inside the Windows / Android apps: never the landing page ----
        for label, platform, kw in (("desktop-app", "desktop", {"viewport": {"width": 1280, "height": 800}}),
                                    ("android-app", "android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True})):
            ctx = browser.new_context(**kw)
            ctx.add_init_script(f"window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = '{platform}'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';")
            pg = ctx.new_page()
            watch(pg, label)
            seen_landing = []
            pg.on("request", lambda r: r.method == "HEAD" and "/updates/" in r.url and seen_landing.append(r.url))
            pg.goto(BASE + "/")
            pg.wait_for_selector("#auth:not([hidden])")
            assert path_of(pg) == "/app", f"{label}: {pg.url}"
            assert not pg.query_selector("#stage"), f"{label}: landing page showed"
            assert not seen_landing, f"{label}: landing page ran: {seen_landing}"
            assert not pg.query_selector("#auth-home"), f"{label}: 'What's Outhouse?' link inside the app"
            shot(pg, f"233-{label}-no-landing")
            ctx.close()

        # ---- signed in: straight to the app ----
        if token:
            for label, kw, n in (("desktop", {"viewport": {"width": 1280, "height": 800}}, 234),
                                 ("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 235)):
                ctx = browser.new_context(**kw)
                ctx.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)});")
                pg = ctx.new_page()
                watch(pg, f"signed-in-{label}")
                pg.goto(BASE + "/")
                pg.wait_for_selector("#app:not([hidden])", timeout=15000)
                assert path_of(pg) == "/app", pg.url
                assert not pg.query_selector("#stage")
                pg.wait_for_timeout(1200)
                shot(pg, f"{n}-signed-in-{label}-redirect")
                ctx.close()

        browser.close()
finally:
    if made_updates:
        shutil.rmtree(updates, ignore_errors=True)
    else:
        for f in ("latest.json", "android.json", "Outhouse-Windows-Setup.exe", "Outhouse-Android.apk"):
            (updates / f).unlink(missing_ok=True)

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("Landing page test passed.")
