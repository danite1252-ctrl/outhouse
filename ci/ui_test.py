"""Clicks through the web client in headless Chrome against a real server.

Fails on any JavaScript error and saves screenshots (uploaded by CI) so the
new screens can be eyeballed without deploying.
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


def wait_for_server():
    for _ in range(60):
        try:
            urllib.request.urlopen(BASE + "/api/health")
            return
        except Exception:
            time.sleep(1)
    sys.exit("server never came up")


wait_for_server()
suffix = str(int(time.time()))[-6:]
carol = api("POST", "/api/auth/register", {"username": f"carol{suffix}", "password": "password123", "display_name": "Carol"})
dave = api("POST", "/api/auth/register", {"username": f"dave{suffix}", "password": "password123", "display_name": "Dave"})
TOKEN = carol["token"]
guild = api("POST", "/api/guilds", {"name": "UI Test Guild"}, TOKEN)
chan = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "raids"}, TOKEN)
code = api("POST", f"/api/guilds/{guild['id']}/invites", None, TOKEN)["code"]
api("POST", f"/api/invites/{code}", None, dave["token"])
api("PATCH", "/api/users/@me", {"about_me": "Holy priest, will heal for gold.", "profile_theme": "moonwell"}, TOKEN)
api("POST", f"/api/channels/{chan['id']}/messages", {"content": "Raid tonight, sign up below"}, TOKEN)
start = time.strftime("%Y-%m-%dT20:00:00Z", time.gmtime(time.time() + 2 * 86400))
raid = api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "Molten Core", "description": "Bring flasks and food.", "starts_at": start,
}, TOKEN)
api("PUT", f"/api/events/{raid['event']['id']}/signup",
    {"status": "yes", "role": "tank", "class": "Paladin", "spec": "Protection"}, dave["token"])
api("POST", f"/api/guilds/{guild['id']}/bots", {"name": "Dice"}, TOKEN)

errors = []


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


with sync_playwright() as p:
    browser = p.chromium.launch()

    # ---- Desktop ----
    page = browser.new_page(viewport={"width": 1280, "height": 800})
    watch(page, "desktop")
    page.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    page.goto(BASE + "/")
    page.wait_for_selector(".event-card", timeout=15000)
    page.screenshot(path=f"{SHOTS}/01-raid-card.png")

    # Sign up through the card.
    page.click(".event-card .ev-btn:has-text('Sign up')")
    page.wait_for_selector(".signup-pop")
    page.click(".su-class:has-text('Priest')")
    page.click(".su-spec:has-text('Holy')")
    page.screenshot(path=f"{SHOTS}/02-signup-popover.png")
    page.click(".signup-pop button.primary")
    page.wait_for_selector(".ev-col.healer .su-name.me", timeout=10000)

    # Profile card.
    page.click(".msg-author >> nth=0")
    page.wait_for_selector(".profile-card")
    page.screenshot(path=f"{SHOTS}/03-profile-card.png")
    page.keyboard.press("Escape")

    # Raid list + plan dialog.
    page.click("#raid-btn")
    page.wait_for_selector(".raid-list .raid-row-btn")
    page.screenshot(path=f"{SHOTS}/04-raid-list.png")
    page.click(".raid-list button.primary")
    page.wait_for_selector(".raid-dialog[open]")
    page.screenshot(path=f"{SHOTS}/05-plan-raid.png")
    page.fill(".raid-dialog input[maxlength='100']", "Mythic+ night")
    page.click(".raid-dialog button[type=submit]")
    page.wait_for_selector(".ev-title:has-text('Mythic+ night')", timeout=10000)

    # Profile settings page.
    page.click("#user-settings")
    page.click(".settings-nav button:has-text('Profile & picture')")
    page.wait_for_selector(".theme-grid .theme-swatch")
    page.click(".theme-swatch[title='Felfire']")
    page.screenshot(path=f"{SHOTS}/06-profile-settings.png")
    page.click("button:has-text('Save profile')")
    page.wait_for_timeout(500)
    page.keyboard.press("Escape")

    # Server settings → Bots.
    page.evaluate("openSettings('bots')")
    page.wait_for_selector("text=Bots are accounts your own code controls")
    page.wait_for_selector(".int-item")
    page.screenshot(path=f"{SHOTS}/07-bots.png")
    page.keyboard.press("Escape")

    # Slash command menu needs a running bot's commands; check the box at least opens nothing.
    page.fill("#composer-input", "/")
    page.wait_for_timeout(300)
    page.fill("#composer-input", "")

    # ---- Phone ----
    phone = browser.new_page(viewport={"width": 390, "height": 844}, is_mobile=True, has_touch=True)
    watch(phone, "phone")
    phone.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    phone.goto(BASE + "/")
    phone.wait_for_selector(".event-card", timeout=15000)
    phone.screenshot(path=f"{SHOTS}/08-phone-chat.png")
    phone.click("#nav-toggle")
    phone.wait_for_timeout(400)
    phone.screenshot(path=f"{SHOTS}/09-phone-channels.png")
    phone.click(".mobile-scrim", position={"x": 370, "y": 420})
    phone.click("#members-toggle")
    phone.wait_for_timeout(400)
    phone.screenshot(path=f"{SHOTS}/10-phone-members.png")

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("UI test passed.")
