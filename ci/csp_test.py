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
# A raid with soft reserves: its composition and the loot window run under the policy too.
csp_raid = api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "CSP raid", "starts_at": time.strftime("%Y-%m-%dT20:00:00Z", time.gmtime(time.time() + 2 * 86400)),
    "soft_reserves": 1, "loot_instance": "mc"}, TOKEN)["event"]
api("PUT", f"/api/events/{csp_raid['id']}/signup", {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection"}, TOKEN)
# A bot message with buttons and a select menu (components.js).
csp_bot = api("POST", f"/api/guilds/{guild['id']}/bots", {"name": "CSP Bot"}, TOKEN)
api("POST", f"/api/channels/{chan['id']}/messages", {"content": "Pick a die", "components": [
    {"type": 1, "components": [{"type": 2, "style": 1, "label": "Roll", "emoji": {"name": "🎲"}, "custom_id": "roll"},
                                {"type": 2, "style": 5, "label": "Docs", "url": "https://example.com"}]},
    {"type": 1, "components": [{"type": 3, "custom_id": "size", "placeholder": "Dice size",
                                "options": [{"label": "d6", "value": "6"}, {"label": "d20", "value": "20"}]}]}]}, csp_bot["token"])

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
    pg.wait_for_selector("#messages .msg-components .mc-btn", timeout=10000)
    pg.click("#messages .mc-select")
    pg.wait_for_selector(".mc-pop .mc-option", timeout=5000)
    pg.keyboard.press("Escape")
    pg.evaluate(f"openEditChannel(chById({json.dumps(chan['id'])}), 'automod')")
    pg.wait_for_selector("dialog.edit-channel .am-card", timeout=10000)
    pg.keyboard.press("Escape")
    pg.click("#voice-channels .channel.voice:has-text('CSP Voice')")
    pg.wait_for_selector("#voice-panel .vp-status:not(.connecting)", timeout=20000)
    until(pg, "state.voice.room && state.voice.room.state === 'connected'")
    pg.wait_for_timeout(1500)
    pg.screenshot(path=f"{SHOTS}/csp-voice.png")
    # A voice message recorded while in a voice channel: its own mic stream,
    # the call's mic (and noise gate) carry on as they were.
    gate_track = pg.evaluate("micPipe.stream?.getAudioTracks()[0]?.id || null")
    pg.click("#mic-btn")
    pg.wait_for_selector("#composer .vm-recbar", timeout=10000)
    pg.wait_for_timeout(1500)
    pg.click("#composer .vm-recbar .vm-send")
    pg.wait_for_selector("#messages .voice-msg", timeout=15000)
    until(pg, "state.voice.room && state.voice.room.state === 'connected'")
    assert pg.evaluate("micPipe.stream?.getAudioTracks()[0]?.id || null") == gate_track, "voice message touched the call's mic"
    assert pg.evaluate("micPipe.stream?.getAudioTracks()[0]?.readyState") == "live", "the call's mic stopped"
    pg.click("#messages .voice-msg .vm-play")
    pg.wait_for_timeout(800)
    pg.evaluate("leaveVoice(true)")
    # Stickers (built-in pictures), the forward dialog and Home → Friends.
    pg.click("#emoji-btn")
    pg.click("#sticker-tab")
    pg.wait_for_selector(".sticker-pane .sticker-tile[data-sticker='builtin:gg']", timeout=10000)
    pg.click(".sticker-pane .sticker-tile[data-sticker='builtin:gg']")
    pg.wait_for_selector("#messages .msg-sticker", timeout=10000)
    pg.evaluate("openForwardDialog(state.messages[0])")
    pg.wait_for_selector(".fwd-dialog[open] .fwd-row", timeout=10000)
    pg.keyboard.press("Escape")
    pg.evaluate("openFriends('add')")
    pg.wait_for_selector("#add-friend-input", timeout=10000)
    pg.screenshot(path=f"{SHOTS}/csp-friends.png")
    pg.click(".guild[title='CSP Check']")
    pg.wait_for_selector("#messages .msg", timeout=10000)
    pg.click("#user-settings")
    pg.wait_for_selector("dialog.settings", timeout=10000)
    pg.wait_for_timeout(500)
    pg.keyboard.press("Escape")
    pg.click(".event-card .ev-comp > summary")
    pg.wait_for_selector(".event-card .comp-buff[data-buff='battle_shout'].yes", timeout=10000)
    pg.click(".event-card .ev-link.sr")
    # Boss sections start closed; open Ragnaros's (icons come through our own server).
    pg.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Ragnaros']", timeout=15000)
    pg.click("dialog.loot-dialog .loot-boss[data-boss='Ragnaros'] > summary")
    pg.wait_for_selector("dialog.loot-dialog .loot-row[data-item='16922']", timeout=10000)
    for _ in range(50):
        if pg.evaluate("[...document.querySelectorAll('dialog.loot-dialog .loot-boss[open] img.item-icon')].some(i => i.complete && i.naturalWidth > 0)"):
            break
        pg.wait_for_timeout(200)
    else:
        problems.append("item icons didn't load under the real policy")
    pg.click("dialog.loot-dialog .loot-row[data-item='16922'] .sr-pick")
    pg.wait_for_selector("dialog.loot-dialog .loot-row[data-item='16922'].mine", timeout=10000)
    pg.hover("dialog.loot-dialog .loot-row[data-item='16922'] .item-name")
    pg.wait_for_selector("dialog.loot-dialog .item-tip:has-text('Classes: Priest')", timeout=10000)
    pg.keyboard.press("Escape")
    violations(pg, "app")
    ctx.close()

    # The orange update button in the Windows app (a stand-in app bridge),
    # and "Refresh to update" in a browser, under the real policy.
    status = {"available": True, "version": "0.2.41", "notes": "Faster voice.", "date": "2026-11-09"}

    def fake_bridge(route):
        headers = {"Access-Control-Allow-Origin": BASE}
        if "/update-status" in route.request.url:
            route.fulfill(status=200, headers=headers, content_type="application/json", body=json.dumps(status))
        elif "/update-install" in route.request.url:
            status.update({"downloading": True, "progress": 0.3})
            route.fulfill(status=200, headers=headers, body="ok")
        else:
            route.fulfill(status=200, headers=headers, body="ok")
    ctx = browser.new_context(viewport={"width": 1280, "height": 800})
    ctx.add_init_script(COLLECT + "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'desktop'; window.__OUTHOUSE_APP_VERSION__ = '0.2.40';"
                        + f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    ctx.route("http://outhouse.localhost/**", fake_bridge)
    pg = ctx.new_page()
    watch(pg, "update button")
    pg.goto(BASE + "/app")
    pg.wait_for_selector("#messages .msg", timeout=15000)
    pg.wait_for_selector("#update-btn:not([hidden])", timeout=10000)
    pg.click("#update-btn")
    pg.wait_for_selector(".upd-pop #update-now")
    pg.click(".upd-pop #update-now")
    pg.wait_for_selector(".upd-pop .upd-progress", timeout=10000)
    pg.wait_for_timeout(800)
    pg.screenshot(path=f"{SHOTS}/csp-update-button.png")
    # The web client check: /api/health's web_build, then a changed one.
    until(pg, "!!updates.state.webBuild")
    pg.evaluate("updates.state.webBuild = 'something-older'")
    pg.evaluate("checkForUpdates()")
    until(pg, "!!updates.state.webNew")
    violations(pg, "update button")
    ctx.close()
    browser.close()

if problems:
    print("Problems under the real Content-Security-Policy:")
    for line in problems:
        print("  " + line)
    sys.exit(1)
print("CSP test passed: landing, sign-in, invite page, chat, voice, voice messages, stickers, forwarding, friends, settings, soft reserves and the update button work under the real policy.")
