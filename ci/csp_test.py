"""The web client under its real Content-Security-Policy (no bypass).

ui_test.py and invite_test.py bypass CSP (Playwright's wait_for_function
needs eval), so this walks the main paths with the policy on and fails on
any CSP violation: landing page, sign-in page, the app with chat, sending a
message, voice (LiveKit + the noise-gate worklet), settings, and an invite
page. Polls with page.evaluate (DevTools evaluation isn't subject to CSP).
"""

import json
import os
import sys
import time
import urllib.request

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
os.makedirs(SHOTS, exist_ok=True)


def api(method, path, body=None, token=None):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
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
me = api("POST", "/api/auth/register", {"username": f"csp{suffix}", "password": "password123", "display_name": "Cass"})
TOKEN = me["token"]
guild = api("POST", "/api/guilds", {"name": "CSP Check"}, TOKEN)
chan = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "chat"}, TOKEN)
api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "CSP Voice", "kind": 2}, TOKEN)
api("POST", f"/api/channels/{chan['id']}/messages", {"content": "hello https://example.com <:nope:1>"}, TOKEN)
code = api("POST", f"/api/guilds/{guild['id']}/invites", None, TOKEN)["code"]

problems = []
COLLECT = """
window.__csp = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__csp.push(`${e.effectiveDirective} blocked ${e.blockedURI || '(inline)'} at ${e.sourceFile || ''}:${e.lineNumber || ''}`);
});
"""


def watch(pg, label):
    pg.on("pageerror", lambda e: problems.append(f"[{label}] page error: {e}"))
    pg.on("console", lambda m: m.type == "error" and ("Content Security Policy" in m.text or "Refused to" in m.text)
          and problems.append(f"[{label}] console: {m.text}"))


def until(pg, expr, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        if pg.evaluate(expr):
            return
        time.sleep(0.25)
    raise SystemExit(f"timed out waiting for {expr}")


def violations(pg, label):
    for v in pg.evaluate("window.__csp || []"):
        problems.append(f"[{label}] CSP violation: {v}")


with sync_playwright() as p:
    browser = p.chromium.launch(args=[
        "--use-fake-ui-for-media-stream",
        "--use-fake-device-for-media-stream",
        "--autoplay-policy=no-user-gesture-required",
    ])

    # Signed out: landing page and sign-in page.
    ctx = browser.new_context(viewport={"width": 1280, "height": 800})
    ctx.add_init_script(COLLECT)
    pg = ctx.new_page()
    watch(pg, "landing")
    pg.goto(BASE + "/")
    pg.wait_for_selector("#stage.open", timeout=15000)
    pg.wait_for_timeout(1000)
    violations(pg, "landing")
    pg.goto(BASE + "/login")
    pg.wait_for_selector("#auth-form", timeout=15000)
    violations(pg, "login")
    pg.goto(BASE + f"/invite/{code}")
    pg.wait_for_selector("#inv-title", timeout=15000)
    pg.wait_for_timeout(500)
    violations(pg, "invite page")
    ctx.close()

    # Signed in: chat, voice, settings.
    ctx = browser.new_context(viewport={"width": 1280, "height": 800}, permissions=["microphone"])
    ctx.add_init_script(COLLECT + f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    pg = ctx.new_page()
    watch(pg, "app")
    pg.goto(BASE + "/app")
    pg.wait_for_selector("#app:not([hidden])", timeout=15000)
    pg.wait_for_selector("#messages .msg", timeout=15000)
    until(pg, "!!window.LivekitClient")
    pg.fill("#composer-input", "sent under the real CSP")
    pg.press("#composer-input", "Enter")
    pg.wait_for_selector("#messages .msg:has-text('sent under the real CSP')", timeout=10000)
    pg.click("#voice-channels .channel.voice:has-text('CSP Voice')")
    pg.wait_for_selector("#voice-panel .vp-status:not(.connecting)", timeout=20000)
    until(pg, "state.voice.room && state.voice.room.state === 'connected'")
    pg.wait_for_timeout(1500)
    pg.screenshot(path=f"{SHOTS}/csp-voice.png")
    pg.evaluate("leaveVoice(true)")
    pg.click("#user-settings")
    pg.wait_for_selector("dialog.settings", timeout=10000)
    pg.wait_for_timeout(500)
    pg.keyboard.press("Escape")
    violations(pg, "app")
    ctx.close()
    browser.close()

if problems:
    print("Problems under the real Content-Security-Policy:")
    for line in problems:
        print("  " + line)
    sys.exit(1)
print("CSP test passed: landing, sign-in, invite page, chat, voice and settings work under the real policy.")
