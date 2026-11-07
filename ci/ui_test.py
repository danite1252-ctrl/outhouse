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

# Search, pins and mention counts: an old message beyond the first page, then mentions of Carol.
CAROL = carol["user"]["id"]
general = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "general"}, TOKEN)
api("POST", f"/api/channels/{general['id']}/messages",
    {"content": "Ancient lore: the Onyxia key quest starts in Dustwallow Marsh"}, dave["token"])
for i in range(80):
    api("POST", f"/api/channels/{general['id']}/messages", {"content": f"filler chatter {i}"}, dave["token"])
api("POST", f"/api/channels/{general['id']}/messages", {"content": f"<@{CAROL}> who has the onyxia key?"}, dave["token"])
api("POST", f"/api/channels/{general['id']}/messages", {"content": "@everyone raid at 8, bring flasks"}, dave["token"])
family = api("POST", "/api/guilds", {"name": "Family"}, dave["token"])
fchan = api("POST", f"/api/guilds/{family['id']}/channels", {"name": "chat"}, dave["token"])
fcode = api("POST", f"/api/guilds/{family['id']}/invites", None, dave["token"])["code"]
api("POST", f"/api/invites/{fcode}", None, TOKEN)
api("POST", f"/api/channels/{fchan['id']}/messages", {"content": f"Dinner on Sunday <@{CAROL}>?"}, dave["token"])

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

    # ---- Unread markers and mention counts ----
    def badge_text(selector):
        return page.evaluate(f"document.querySelector({json.dumps(selector)})?.textContent ?? null")

    page.wait_for_selector(".channel:has-text('general') .mention-badge", timeout=10000)
    assert badge_text(".channel.unread .mention-badge") == "2", badge_text(".channel.unread .mention-badge")
    assert badge_text(".guild[title='Family'] .mention-badge") == "1", badge_text(".guild[title='Family'] .mention-badge")
    assert badge_text(".guild[title='UI Test Guild'] .mention-badge") == "2"
    assert page.title() == "(3) Outhouse", page.title()
    # A new mention arrives live.
    api("POST", f"/api/channels/{fchan['id']}/messages", {"content": "@here pizza tonight?"}, dave["token"])
    page.wait_for_function("document.querySelector(\".guild[title='Family'] .mention-badge\")?.textContent === '2'", timeout=10000)
    page.wait_for_function("document.title === '(4) Outhouse'")
    page.screenshot(path=f"{SHOTS}/13-mention-badges.png")
    # Opening the channel clears its count.
    page.click(".channel:has-text('general')")
    page.wait_for_selector(".channel:has-text('general') .mention-badge", state="detached", timeout=10000)
    page.wait_for_function("document.title === '(2) Outhouse'")
    assert page.query_selector(".guild[title='UI Test Guild'] .mention-badge") is None

    # ---- Pins ----
    last = "#messages article.msg >> nth=-1"
    page.hover(last)
    page.click(f"{last} >> .msg-actions button[title='Pin']")
    page.wait_for_selector("#messages article.msg.pinned", timeout=10000)
    page.click("#pins-btn")
    page.wait_for_selector(".pins-panel .sp-card:has-text('raid at 8')", timeout=10000)
    # Pin another from the right-click menu; the open list updates.
    page.click("#messages article.msg >> nth=-2", button="right")
    page.click(".menu button:has-text('Pin message')")
    page.wait_for_function("document.querySelectorAll('.pins-panel .sp-card').length === 2", timeout=10000)
    page.screenshot(path=f"{SHOTS}/14-pins-panel.png")
    page.click(".pins-panel .sp-card:has-text('who has the onyxia key')")
    page.wait_for_selector("article.msg.flash")
    page.click(".pins-panel .sp-head button[title='Close']")

    # ---- Search ----
    page.fill("#search-input", "onyxia key")
    page.press("#search-input", "Enter")
    page.wait_for_selector(".search-panel .sp-card", timeout=10000)
    page.wait_for_function("document.querySelectorAll('.search-panel .sp-card').length === 2")
    assert page.query_selector(".search-panel mark") is not None
    page.screenshot(path=f"{SHOTS}/15-search-results.png")
    # The old message is beyond the first page: it loads the messages around it.
    page.click(".search-panel .sp-card:has-text('Ancient lore')")
    page.wait_for_selector("#detached-bar:not([hidden])", timeout=10000)
    page.wait_for_selector("article.msg.flash:has-text('Ancient lore')")
    page.screenshot(path=f"{SHOTS}/16-search-jump.png")
    page.click("#detached-bar button:has-text('Jump to present')")
    page.wait_for_selector("#detached-bar", state="hidden", timeout=10000)
    page.wait_for_selector("article.msg:has-text('raid at 8')")
    # Filters.
    page.fill("#search-input", "in:#raids sign")
    page.press("#search-input", "Enter")
    page.wait_for_function("document.querySelector('.search-panel .sp-count')?.textContent === '1 result'", timeout=10000)
    page.fill("#search-input", "from:@nobody zzz")
    page.press("#search-input", "Enter")
    page.wait_for_selector(".search-panel .sp-empty", timeout=10000)
    page.press("#search-input", "Escape")
    page.wait_for_selector(".search-panel", state="detached")

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
    phone.click(".mobile-scrim", position={"x": 20, "y": 420})
    phone.wait_for_timeout(300)
    phone.click("#search-toggle")
    phone.fill(".sp-search input", "onyxia")
    phone.press(".sp-search input", "Enter")
    phone.wait_for_selector(".search-panel .sp-card", timeout=10000)
    phone.screenshot(path=f"{SHOTS}/17-phone-search.png")
    phone.click("#pins-btn")
    phone.wait_for_selector(".pins-panel .sp-empty", timeout=10000)
    phone.screenshot(path=f"{SHOTS}/18-phone-pins.png")
    phone.click(".pins-panel .sp-head button[title='Close']")

    # ---- Android app (draws under the status bar and navigation buttons) ----
    droid = browser.new_page(viewport={"width": 393, "height": 852}, is_mobile=True, has_touch=True)
    watch(droid, "android")
    droid.add_init_script(f"window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1'; localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    droid.goto(BASE + "/")
    droid.wait_for_selector(".event-card", timeout=15000)
    droid.screenshot(path=f"{SHOTS}/11-android-chat.png")
    droid.evaluate("openUserSettings('voice')")
    droid.wait_for_timeout(600)
    droid.screenshot(path=f"{SHOTS}/12-android-settings.png")

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("UI test passed.")
