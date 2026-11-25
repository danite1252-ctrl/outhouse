"""The Android app's in-app update (web/updates.js) with a stand-in app bridge,
under the real Content-Security-Policy, on desktop and phone sizes:

- "Update now" asks the app to download the version-numbered APK
  (/apk-update-start), shows progress (percent, MB) and Cancel works.
- "Install unknown apps" not allowed yet: the explanation and Open settings.
- A download that fails the app's checks: the safety-check message.
- An older app without /apk-update-* (404): the old open-in-browser download.

Screenshots: apk-update-*.png. Fails on JavaScript errors or CSP violations.
"""

import json
import os
import sys
import time
import urllib.parse
import urllib.request

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
PREFIX = os.environ.get("SHOT_PREFIX", "apk-update")
os.makedirs(SHOTS, exist_ok=True)
NEW = "0.2.999"
APK = f"{BASE}/updates/Outhouse-Android-{NEW}.apk"
MB = 1048576
FAKE_IP = f"198.18.98.{int(time.time()) % 250 + 1}"


def api(method, path, body=None, token=None):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
    # Its own address, so the earlier tests' sign-ups don't hit the rate limit.
    req.add_header("X-Forwarded-For", FAKE_IP)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    data = json.dumps(body).encode() if body is not None else None
    with urllib.request.urlopen(req, data) as res:
        text = res.read().decode()
        return json.loads(text) if text else None


for _ in range(60):
    try:
        urllib.request.urlopen(BASE + "/api/health")
        break
    except Exception:
        time.sleep(1)

suffix = str(int(time.time()))[-6:]
me = api("POST", "/api/auth/register", {"username": f"apkup{suffix}", "email": "someone@example.test",
                                       "password": "password123", "birth_date": "1990-01-01", "display_name": "Andy"})
TOKEN = me["token"]
guild = api("POST", "/api/guilds", {"name": "Phone Updates"}, TOKEN)
chan = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "chat"}, TOKEN)
api("POST", f"/api/channels/{chan['id']}/messages", {"content": "new build is out"}, TOKEN)

problems = []
COLLECT = """
window.__csp = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__csp.push(`${e.effectiveDirective} blocked ${e.blockedURI || '(inline)'}`);
});
window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';
"""


def until(pg, expr, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        if pg.evaluate(expr):
            return
        time.sleep(0.2)
    raise SystemExit(f"timed out waiting for {expr}")


def check(cond, what):
    if not cond:
        problems.append(what)


class Bridge:
    """The app's http://outhouse.localhost bridge, as the Android app answers it."""

    def __init__(self, native=True):
        self.native = native
        self.job = {"state": "idle", "received": 0, "total": 0, "version": "", "canInstall": True}
        self.calls = []

    def __call__(self, route):
        url = urllib.parse.urlparse(route.request.url)
        q = dict(urllib.parse.parse_qsl(url.query))
        self.calls.append((url.path, q))
        headers = {"Access-Control-Allow-Origin": BASE, "Cache-Control": "no-store"}
        if url.path.startswith("/apk-update-"):
            if not self.native:
                return route.fulfill(status=404, headers=headers, body="unknown")
            if url.path == "/apk-update-start":
                self.job.update(state="downloading", received=int(6.1 * MB), total=int(14.5 * MB), version=q.get("version", ""))
            elif url.path == "/apk-update-cancel":
                self.job.update(state="idle", received=0, total=0, note="cancelled")
            return route.fulfill(status=200, headers=headers, content_type="application/json", body=json.dumps(self.job))
        route.fulfill(status=200, headers=headers, body="ok")

    def called(self, path):
        return [q for p, q in self.calls if p == path]


def open_app(browser, size, bridge, label):
    ctx = browser.new_context(**size)
    ctx.add_init_script(COLLECT + f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); "
                        f"localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); "
                        f"localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    ctx.route("http://outhouse.localhost/**", bridge)
    ctx.route("**/updates/android.json", lambda r: r.fulfill(
        status=200, content_type="application/json",
        body=json.dumps({"version": NEW, "url": APK, "notes": "Updates now install inside the app."})))
    pg = ctx.new_page()
    pg.on("pageerror", lambda e: problems.append(f"[{label}] page error: {e}"))
    pg.on("console", lambda m: m.type == "error" and "Failed to load resource" not in m.text
          and problems.append(f"[{label}] console: {m.text}"))
    pg.goto(BASE + "/app")
    pg.wait_for_selector("#messages .msg", timeout=20000)
    pg.wait_for_selector("#update-btn:not([hidden])", timeout=15000)
    return ctx, pg


def popover(pg):
    if not pg.query_selector(".upd-pop"):
        pg.click("#update-btn")
    pg.wait_for_selector(".upd-pop", timeout=5000)


def refresh(pg):
    # What the app does when you come back to it: ask the bridge again.
    pg.evaluate("window.dispatchEvent(new Event('focus'))")


with sync_playwright() as p:
    browser = p.chromium.launch()
    sizes = {
        "desktop": {"viewport": {"width": 1280, "height": 800}},
        "phone": {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True},
    }
    for name, size in sizes.items():
        bridge = Bridge()
        ctx, pg = open_app(browser, size, bridge, name)
        until(pg, "updates.state.apkNative === true")
        popover(pg)
        check(pg.inner_text(".upd-pop #update-now") == "Update now", f"[{name}] the button isn't 'Update now'")
        pg.click(".upd-pop #update-now")
        pg.wait_for_selector(".upd-pop .upd-progress", timeout=5000)
        until(pg, "document.querySelector('.upd-pop .upd-status')?.textContent.includes('42%')")
        status = pg.inner_text(".upd-pop .upd-status")
        check("6.1 of 14.5 MB" in status, f"[{name}] progress text: {status!r}")
        start = bridge.called("/apk-update-start")
        check(start and start[0].get("url") == APK and start[0].get("version") == NEW,
              f"[{name}] start got {start}")
        check(not bridge.called("/open"), f"[{name}] opened the browser too")
        pg.wait_for_timeout(400)
        pg.screenshot(path=f"{SHOTS}/{PREFIX}-{name}-progress.png")
        pg.click(".upd-pop #update-cancel")
        pg.wait_for_selector(".upd-pop #update-now", timeout=5000)
        check(bridge.called("/apk-update-cancel"), f"[{name}] Cancel didn't reach the app")

        # "Install unknown apps" isn't allowed for Outhouse yet.
        bridge.job.update(state="needs_permission", received=int(14.5 * MB), total=int(14.5 * MB), canInstall=False)
        refresh(pg)
        popover(pg)
        pg.wait_for_selector(".upd-pop #update-permission", timeout=5000)
        check("Allow from this source" in pg.inner_text(".upd-pop #update-permission"), f"[{name}] permission text")
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{PREFIX}-{name}-permission.png")
        pg.click(".upd-pop #update-settings")
        until(pg, "true")
        pg.wait_for_timeout(300)
        check(bridge.called("/apk-update-settings"), f"[{name}] Open settings didn't reach the app")
        # Allowed now: the app carries on by itself (the page keeps polling).
        bridge.job.update(state="confirm", canInstall=True)
        until(pg, "document.querySelector('.upd-pop .upd-status')?.textContent.includes(\"Android's prompt\")")

        # A download that isn't ours: deleted, and said so.
        bridge.job.update(state="error", code="mismatch", error="The download isn't signed with Outhouse's key.")
        refresh(pg)
        pg.wait_for_selector(".upd-pop #update-error", timeout=5000)
        err = pg.inner_text(".upd-pop #update-error")
        check("safety check" in err and "signed with Outhouse's key" in err, f"[{name}] mismatch text: {err!r}")
        pg.screenshot(path=f"{SHOTS}/{PREFIX}-{name}-mismatch.png")
        for v in pg.evaluate("window.__csp || []"):
            problems.append(f"[{name}] CSP violation: {v}")
        ctx.close()

    # An older Android app (no /apk-update-*): the download opens in the browser.
    bridge = Bridge(native=False)
    ctx, pg = open_app(browser, sizes["phone"], bridge, "older app")
    until(pg, "updates.state.apkNative === false")
    popover(pg)
    check(pg.inner_text(".upd-pop #update-now") == "Download", "older app: the button isn't 'Download'")
    pg.click(".upd-pop #update-now")
    until(pg, "true")
    for _ in range(25):
        if bridge.called("/open"):
            break
        pg.wait_for_timeout(200)
    opened = bridge.called("/open")
    check(opened and opened[0].get("url") == APK, f"older app: opened {opened}")
    ctx.close()
    browser.close()

if problems:
    print("\n".join(problems))
    sys.exit(1)
print("APK update UI test passed: progress + cancel, permission, safety-check error, older-app fallback (desktop and phone).")
