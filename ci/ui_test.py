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

# ---- Batch 2: DMs, threads, polls, custom emoji ----
import struct
import zlib


def make_png(size=32):
    """A little orange smiley, so custom emoji look like something in screenshots."""
    c = (size - 1) / 2
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            dx, dy = x - c, y - c
            d = (dx * dx + dy * dy) ** 0.5
            if d > size / 2 - 1:
                row += bytes((0, 0, 0, 0))
            elif (abs(abs(dx) - 6) < 2.5 and abs(dy + 4) < 3) or (4 < dy < 8 and abs(dx) < 7):
                row += bytes((40, 20, 10, 255))
            else:
                row += bytes((255, 179, 71, 255))
        rows.append(bytes(row))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b""))


def upload(path, fields, files, token):
    boundary = "----outhouse" + suffix
    body = b""
    for k, v in fields.items():
        body += f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
    for k, (fname, data, ctype) in files.items():
        body += (f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"; filename="{fname}"\r\n'
                 f"Content-Type: {ctype}\r\n\r\n").encode() + data + b"\r\n"
    body += f"--{boundary}--\r\n".encode()
    req = urllib.request.Request(BASE + path, data=body, method="POST")
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req) as res:
        return json.loads(res.read().decode())


erin = api("POST", "/api/auth/register", {"username": f"erin{suffix}", "password": "password123", "display_name": "Erin"})
api("POST", f"/api/invites/{code}", None, erin["token"])
upload(f"/api/guilds/{guild['id']}/emojis", {"name": "pog"}, {"file": ("pog.png", make_png(), "image/png")}, TOKEN)
api("POST", f"/api/channels/{chan['id']}/messages", {"content": "Who's bringing fire resist gear?"}, dave["token"])
api("POST", f"/api/channels/{chan['id']}/messages", {"content": ":pog: gg on the Rag kill"}, dave["token"])

# ---- Batch 3: raid extras (repeating raids, deadlines, bench, attendance) ----
DAVE = dave["user"]["id"]
ERIN = erin["user"]["id"]
iso = lambda t: time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))
tomorrow = time.gmtime(time.time() + 86400)
api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "BWL weekly", "description": "Blackwing Lair, full clear.", "starts_at": time.strftime("%Y-%m-%dT19:30:00Z", tomorrow),
    "max_tanks": 2, "max_healers": 4, "max_dps": 14, "signup_deadline_minutes": 120,
    "repeat": {"weekdays": [tomorrow.tm_wday, (tomorrow.tm_wday + 2) % 7], "every_weeks": 1, "count": 8, "time_zone": "UTC"},
}, TOKEN)
locked = api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "Onyxia (sign-ups closed)", "starts_at": iso(time.time() + 2 * 3600), "signup_deadline_minutes": 180,
    "max_tanks": 1, "max_healers": 2, "max_dps": 7,
}, TOKEN)
api("PUT", f"/api/events/{locked['event']['id']}/signup",
    {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection", "user_id": DAVE}, TOKEN)
bench = api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "Bench raid", "starts_at": iso(time.time() + 3 * 86400), "max_tanks": 1, "max_healers": 1, "max_dps": 3,
}, TOKEN)
api("PUT", f"/api/events/{bench['event']['id']}/signup", {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection"}, dave["token"])
api("PUT", f"/api/events/{bench['event']['id']}/signup", {"status": "yes", "role": "tank", "class": "Druid", "spec": "Feral (Bear)", "note": "can swap to cat"}, erin["token"])
earlier = api("POST", f"/api/channels/{chan['id']}/events", {"title": "Zul'Gurub warm-up", "starts_at": iso(time.time() - 50 * 60)}, TOKEN)
api("PUT", f"/api/events/{earlier['event']['id']}/attendance",
    {"records": [{"user_id": DAVE, "status": "present"}, {"user_id": ERIN, "status": "absent"}]}, TOKEN)
past = api("POST", f"/api/channels/{chan['id']}/events", {"title": "Molten Core run", "starts_at": iso(time.time() - 40 * 60), "duration_minutes": 180}, TOKEN)
api("PUT", f"/api/events/{past['event']['id']}/signup", {"status": "yes", "role": "dps", "class": "Rogue", "spec": "Combat"}, dave["token"])
api("PUT", f"/api/events/{past['event']['id']}/signup", {"status": "yes", "role": "healer", "class": "Priest", "spec": "Holy"}, erin["token"])

def scroll_to_card(pg, title):
    """Scrolls a raid card into view (by title; cards get redrawn, so no element handles)."""
    pg.evaluate("t => [...document.querySelectorAll('#messages .event-card')].find((c) => c.querySelector('.ev-title')?.textContent.includes(t))?.scrollIntoView({block: 'center'})", title)
    pg.wait_for_timeout(250)


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

    # ---- Scroll-back: older messages load at the top ----
    page.click(".channel:has-text('general')")
    page.wait_for_selector("#messages article.msg:has-text('raid at 8')", timeout=10000)
    assert page.query_selector("#messages article.msg:has-text('Ancient lore')") is None
    page.wait_for_timeout(500)
    page.evaluate("document.getElementById('messages').scrollTop = 0")
    page.wait_for_selector("#messages article.msg:has-text('Ancient lore')", timeout=10000)
    page.wait_for_selector("#messages .history-start")
    top = page.evaluate("document.getElementById('messages').scrollTop")
    assert top > 100, f"scroll position jumped to {top}"
    page.screenshot(path=f"{SHOTS}/19-scrollback.png")

    # ---- Threads ----
    page.click(".channel:has-text('raids')")
    fire = "#messages article.msg:has-text('fire resist')"
    page.wait_for_selector(fire, timeout=10000)
    page.hover(fire)
    page.click(f"{fire} .msg-actions .thread-btn")
    page.wait_for_selector("dialog[open] input")
    page.fill("dialog[open] input", "Fire resist gear")
    page.click("dialog[open] button.primary")
    box = ".thread-panel .thread-composer textarea"
    page.wait_for_selector(box, timeout=10000)
    page.fill(box, "I've got 5 pieces, need the cloak")
    page.press(box, "Enter")
    page.wait_for_selector(".thread-panel article.tmsg:has-text('need the cloak')", timeout=10000)
    page.wait_for_selector("#messages .thread-link:has-text('1 reply')", timeout=10000)
    thread_id = page.evaluate("thread.info.id")
    api("POST", f"/api/channels/{chan['id']}/messages", {"content": "Nice, the Onyxia cloak is a must", "thread_id": thread_id}, dave["token"])
    page.wait_for_selector(".thread-panel article.tmsg:has-text('Onyxia cloak')", timeout=10000)
    page.wait_for_selector("#messages .thread-link:has-text('2 replies')", timeout=10000)
    assert page.query_selector("#messages article.msg:has-text('Onyxia cloak is a must')") is None, "thread reply leaked into the channel"
    page.screenshot(path=f"{SHOTS}/20-thread-panel.png")
    page.click("#threads-btn")
    page.wait_for_selector(".threads-panel .thread-card:has-text('Fire resist gear')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/21-threads-list.png")
    page.click(".threads-panel .thread-card:has-text('Fire resist gear')")
    page.wait_for_selector(".thread-panel article.tmsg:has-text('Onyxia cloak')", timeout=10000)
    page.click(".thread-panel .sp-head button[title='Close']")

    # ---- Polls ----
    page.click("#poll-btn")
    page.wait_for_selector(".poll-dialog[open]")
    page.fill(".poll-dialog input[aria-label='Question']", "Which raid this week?")
    page.fill(".poll-dialog .poll-edit-row >> nth=0 >> input", "Molten Core")
    page.fill(".poll-dialog .poll-edit-row >> nth=1 >> input", "Blackwing Lair")
    page.click(".poll-dialog button:has-text('Add option')")
    page.fill(".poll-dialog .poll-edit-row >> nth=2 >> input", "Onyxia")
    page.check(".poll-dialog .check-row input")
    page.screenshot(path=f"{SHOTS}/22-poll-dialog.png")
    page.click(".poll-dialog button[type=submit]")
    poll = "#messages .poll-card:has-text('Which raid this week?')"
    page.wait_for_selector(poll, timeout=10000)
    page.click(f"{poll} .poll-opt:has-text('Blackwing Lair')")
    page.wait_for_selector(f"{poll} .poll-opt.mine:has-text('Blackwing Lair')", timeout=10000)
    poll_id = page.evaluate("[...state.messages].reverse().find((m) => m.poll).id")
    api("PUT", f"/api/channels/{chan['id']}/polls/{poll_id}/votes", {"options": [1, 2]}, dave["token"])
    page.wait_for_selector(f"{poll} .poll-foot:has-text('2 votes')", timeout=10000)
    page.wait_for_selector(f"{poll} .poll-opt:has-text('Blackwing Lair') .poll-count:has-text('2 · 100%')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/23-poll.png")
    page.click(f"{poll} button:has-text('See votes')")
    page.wait_for_selector(".poll-voters .pv-person:has-text('Dave')")
    page.keyboard.press("Escape")
    # /poll opens the same dialog with the question filled in.
    page.fill("#composer-input", "/poll Pizza or pasta?")
    page.press("#composer-input", "Enter")
    page.wait_for_selector(".poll-dialog[open]")
    assert page.input_value(".poll-dialog input[aria-label='Question']") == "Pizza or pasta?"
    page.keyboard.press("Escape")
    page.wait_for_selector(".poll-dialog", state="detached")

    # ---- Custom emoji ----
    page.wait_for_selector("#messages .msg-text img.custom-emoji[alt=':pog:']", timeout=10000)
    pog = "#messages article.msg:has(img.custom-emoji)"
    page.hover(pog)
    page.click(f"{pog} .msg-actions button[title='Add reaction']")
    page.click(".emoji-tabs .custom-tab")
    page.wait_for_selector(".custom-pane button[title=':pog:']")
    page.screenshot(path=f"{SHOTS}/24-emoji-picker.png")
    page.click(".custom-pane button[title=':pog:']")
    page.wait_for_selector(f"{pog} .reaction.me img.custom-emoji", timeout=10000)
    page.click("#emoji-btn")
    page.click(".emoji-tabs .custom-tab")
    page.click(".custom-pane button[title=':pog:']")
    assert ":pog:" in page.input_value("#composer-input")
    page.press("#composer-input", "Enter")
    page.wait_for_selector("#messages article.msg >> nth=-1 >> img.custom-emoji", timeout=10000)
    page.evaluate("openSettings('emoji')")
    page.wait_for_selector(".emoji-item:has-text(':pog:')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/25-emoji-settings.png")
    page.keyboard.press("Escape")

    # ---- Direct messages ----
    page.click("#messages .msg-author:has-text('Dave') >> nth=0")
    page.wait_for_selector(".profile-card .pc-message")
    page.click(".profile-card .pc-message")
    page.wait_for_selector("#app.home-mode", timeout=10000)
    page.wait_for_selector("#dm-list .dm-row.active:has-text('Dave')", timeout=10000)
    page.fill("#composer-input", "G'day Dave, got a sec?")
    page.press("#composer-input", "Enter")
    page.wait_for_selector("#messages article.msg:has-text('got a sec')", timeout=10000)
    dm_id = page.evaluate("state.channelId")
    api("POST", f"/api/channels/{dm_id}/messages", {"content": "Yeah mate, what's up?"}, dave["token"])
    page.wait_for_selector("#messages article.msg:has-text('Yeah mate')", timeout=10000)
    # A DM while you're in an outhouse: the Home button gets a count.
    page.click(".guild[title='UI Test Guild']")
    page.wait_for_selector("#app:not(.home-mode)")
    page.wait_for_selector(".channel:has-text('raids')")
    api("POST", f"/api/channels/{dm_id}/messages", {"content": "Bring snacks for the raid"}, dave["token"])
    # (Counts every DM message you haven't seen: "Yeah mate" too if this headless window isn't focused.)
    page.wait_for_function("Number(document.querySelector('#home-btn .mention-badge')?.textContent) >= 1", timeout=10000)
    page.screenshot(path=f"{SHOTS}/26-dm-badge.png")
    page.click("#home-btn")
    page.wait_for_selector("#messages article.msg:has-text('Bring snacks')", timeout=10000)
    page.wait_for_selector("#home-btn .mention-badge", state="detached", timeout=10000)
    # Group DM.
    page.click("#new-dm")
    page.wait_for_selector(".people-dialog[open] .person-row")
    page.click(".people-dialog .person-row:has-text('Dave')")
    page.click(".people-dialog .person-row:has-text('Erin')")
    page.fill(".people-dialog input[placeholder='Group name (optional)']", "Raid leads")
    page.screenshot(path=f"{SHOTS}/27-new-group.png")
    page.click(".people-dialog button[type=submit]")
    page.wait_for_selector("#dm-list .dm-row.active:has-text('Raid leads')", timeout=10000)
    page.fill("#composer-input", "Group chat for the raid leads")
    page.press("#composer-input", "Enter")
    page.wait_for_selector("#messages article.msg:has-text('Group chat for the raid leads')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/28-group-dm.png")
    page.click("#dm-tools")
    page.click(".menu button:has-text('Rename group')")
    page.fill("dialog[open] input", "Officers")
    page.click("dialog[open] button.primary")
    page.wait_for_selector("#dm-list .dm-row:has-text('Officers')", timeout=10000)
    page.click("#dm-tools")
    page.click(".menu button:has-text('Leave group')")
    page.click("dialog[open] button.primary")
    page.wait_for_selector("#dm-list .dm-row:has-text('Officers')", state="detached", timeout=10000)
    page.wait_for_selector("#dm-list .dm-row.active:has-text('Dave')", timeout=10000)

    # ---- Raid extras: repeating raids, deadlines, bench, calendar, attendance ----
    page.click(".guild[title='UI Test Guild']")
    page.wait_for_selector("#app:not(.home-mode)")
    page.click(".channel:has-text('raids')")
    bench_card = ".event-card:has-text('Bench raid')"
    page.wait_for_selector(f"{bench_card} .ev-bench li:has-text('Erin')", timeout=10000)
    assert "Dave" in page.inner_text(f"{bench_card} .ev-col.tank")
    scroll_to_card(page, "Bench raid")
    page.screenshot(path=f"{SHOTS}/40-bench-card.png")
    locked_card = ".event-card:has-text('sign-ups closed')"
    page.wait_for_selector(f"{locked_card} .ev-deadline.locked")
    scroll_to_card(page, "sign-ups closed")
    page.screenshot(path=f"{SHOTS}/41-locked-card.png")
    series_card = ".event-card:has-text('BWL weekly')"
    page.wait_for_selector(f"{series_card} .ev-repeat", timeout=10000)
    page.wait_for_selector(f"{series_card} .ev-deadline")
    # A repeating raid's menu offers "this one" and "this and future".
    scroll_to_card(page, "BWL weekly")
    page.click(f"{series_card} >> nth=0 >> button[title='Raid options']")
    page.wait_for_selector(".menu button:has-text('Edit this and future raids')")
    page.screenshot(path=f"{SHOTS}/42-series-menu.png")
    page.click(".menu button:has-text('Edit this and future raids')")
    page.wait_for_selector(".raid-dialog[open] h2:has-text('Edit this and future raids')")
    page.fill(".raid-dialog input[maxlength='100']", "BWL weekly (Tuesday crew)")
    page.click(".raid-dialog button[type=submit]")
    page.wait_for_selector(".event-card:has-text('BWL weekly (Tuesday crew)')", timeout=10000)
    # Plan a new repeating raid from the dialog.
    page.click("#raid-btn")
    page.wait_for_selector(".raid-list .rl-tool")
    page.screenshot(path=f"{SHOTS}/43-raid-list.png")
    page.click(".raid-list button.primary")
    page.wait_for_selector(".raid-dialog[open]")
    page.select_option(".raid-dialog select[aria-label='Repeat']", "1")
    page.wait_for_selector(".raid-dialog .repeat-box:not([hidden])")
    page.click(".raid-dialog .day-chip:not(.on) >> nth=0")
    page.select_option(".raid-dialog select[aria-label='Sign-ups close']", "1440")
    page.fill(".raid-dialog input[maxlength='100']", "Zul'Gurub weekly")
    page.screenshot(path=f"{SHOTS}/44-plan-repeating.png")
    page.click(".raid-dialog button[type=submit]")
    page.wait_for_selector(".event-card:has-text(\"Zul'Gurub weekly\") .ev-repeat", timeout=10000)
    # Calendar.
    page.click("#raid-btn")
    page.click(".raid-list .rl-tool:has-text('Calendar')")
    page.wait_for_selector(".calendar-dialog .cal-chip", timeout=10000)
    page.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    page.screenshot(path=f"{SHOTS}/45-calendar.png")
    tomorrow_key = page.evaluate("localDateValue(new Date(Date.now() + 86400000))")
    page.click(f".calendar-dialog .cal-day[data-day='{tomorrow_key}']")
    page.wait_for_selector(".calendar-dialog .cal-daypanel .event-card:has-text('BWL weekly')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/46-calendar-day.png")
    # Sign up from the calendar (the card isn't in chat yet for later weeks: it still works).
    page.click(".calendar-dialog .cal-daypanel .event-card >> nth=0 >> .ev-btn:has-text('Sign up')")
    page.wait_for_selector(".calendar-dialog .signup-pop")
    page.click(".calendar-dialog .su-class:has-text('Mage')")
    page.click(".calendar-dialog .su-spec:has-text('Frost')")
    page.click(".calendar-dialog .signup-pop button.primary")
    page.wait_for_selector(".calendar-dialog .cal-daypanel .ev-col.dps .su-name.me", timeout=10000)
    page.click(".calendar-dialog button[title='Next month']")
    page.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    page.click(".calendar-dialog button[title='Close']")
    page.wait_for_selector(".calendar-dialog", state="detached")
    # Take attendance after the raid starts.
    past_card = ".event-card:has-text('Molten Core run')"
    scroll_to_card(page, "Molten Core run")
    page.click(f"{past_card} .ev-btn.attendance")
    page.wait_for_selector(".attendance-dialog .att-row:has-text('Erin')")
    page.click(".attendance-dialog .att-all")
    page.click(".attendance-dialog .att-row:has-text('Erin') .att-choice.late")
    page.wait_for_selector(".attendance-dialog .att-row:has-text('Dave') .att-choice.present.on")
    page.screenshot(path=f"{SHOTS}/47-take-attendance.png")
    page.click(".attendance-dialog button:has-text('Save attendance')")
    page.wait_for_selector(".attendance-dialog", state="detached")
    page.wait_for_selector(f"{past_card} .ev-btn.attendance.done", timeout=10000)
    # The outhouse's attendance table.
    page.click("#raid-btn")
    page.click(".raid-list .rl-tool:has-text('Attendance')")
    page.wait_for_selector(".attendance-table-dialog .att-table tbody tr", timeout=10000)
    assert page.inner_text(".attendance-table-dialog .att-table tbody tr:has-text('Dave') td.pct") == "100%"
    assert page.inner_text(".attendance-table-dialog .att-table tbody tr:has-text('Erin') td.pct") == "50%"
    page.click(".attendance-table-dialog th:has-text('Member')")
    page.wait_for_selector(".attendance-table-dialog th.sorted:has-text('Member')")
    page.screenshot(path=f"{SHOTS}/48-attendance-table.png")
    page.click(".attendance-table-dialog button[title='Close']")
    page.wait_for_selector(".attendance-table-dialog", state="detached")
    # Attendance on a profile card.
    page.evaluate(f"openProfile({json.dumps(ERIN)}, document.querySelector('#messages .event-card'))")
    page.wait_for_selector(".profile-card .pc-attendance:has-text('50%')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/49-profile-attendance.png")
    page.keyboard.press("Escape")

    # ---- Phone ----
    phone = browser.new_page(viewport={"width": 390, "height": 844}, is_mobile=True, has_touch=True)
    watch(phone, "phone")
    phone.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
    phone.goto(BASE + "/")
    phone.wait_for_selector(".event-card", timeout=15000)
    phone.wait_for_selector("#nav-toggle.has-mentions", timeout=10000)  # Family still has mentions
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
    # Poll and thread on a phone.
    phone.wait_for_selector("#messages .poll-card", timeout=10000)
    phone.evaluate("document.querySelector('#messages .poll-card').scrollIntoView({block: 'center'})")
    phone.screenshot(path=f"{SHOTS}/29-phone-poll.png")
    phone.click("#threads-btn")
    phone.wait_for_selector(".threads-panel .thread-card", timeout=10000)
    phone.click(".threads-panel .thread-card:has-text('Fire resist gear')")
    phone.wait_for_selector(".thread-panel article.tmsg:has-text('Onyxia cloak')", timeout=10000)
    phone.screenshot(path=f"{SHOTS}/30-phone-thread.png")
    phone.click(".thread-panel .sp-head button[title='Close']")
    # DMs on a phone: Home is at the top of the outhouse list.
    phone.click("#nav-toggle")
    phone.wait_for_timeout(400)
    phone.click("#home-btn")
    phone.wait_for_selector("#dm-list .dm-row:has-text('Dave')", timeout=10000)
    phone.wait_for_timeout(300)
    phone.screenshot(path=f"{SHOTS}/31-phone-dm-list.png")
    phone.click("#dm-list .dm-row:has-text('Dave')")
    phone.wait_for_selector("#messages article.msg:has-text('Bring snacks')", timeout=10000)
    phone.wait_for_timeout(400)
    phone.screenshot(path=f"{SHOTS}/32-phone-dm-chat.png")
    # Raid calendar (agenda list) and a bench card on a phone.
    phone.click("#nav-toggle")
    phone.wait_for_timeout(400)
    phone.click(".guild[title='UI Test Guild']")
    phone.wait_for_timeout(300)
    if phone.evaluate("document.getElementById('app').classList.contains('show-nav')"):
        phone.click(".channel:has-text('raids')")
    phone.wait_for_selector(".event-card:has-text('Bench raid')", timeout=10000)
    phone.wait_for_timeout(400)
    scroll_to_card(phone, "Bench raid")
    phone.screenshot(path=f"{SHOTS}/50-phone-bench-card.png")
    phone.click("#raid-btn")
    phone.wait_for_selector(".raid-list .rl-tool")
    phone.screenshot(path=f"{SHOTS}/51-phone-raid-list.png")
    phone.click(".raid-list .rl-tool:has-text('Calendar')")
    phone.wait_for_selector(".calendar-dialog .cal-agenda", timeout=10000)
    phone.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    if not phone.query_selector(".calendar-dialog .cal-agenda-row"):
        phone.click(".calendar-dialog button[title='Next month']")
    phone.wait_for_selector(".calendar-dialog .cal-agenda-row", timeout=10000)
    phone.screenshot(path=f"{SHOTS}/52-phone-calendar.png")
    phone.click(".calendar-dialog .cal-agenda-row >> nth=0")
    phone.wait_for_selector(".calendar-dialog .cal-agenda-item.open .event-card")
    phone.screenshot(path=f"{SHOTS}/53-phone-calendar-open.png")
    phone.click(".calendar-dialog button[title='Close']")
    phone.click("#raid-btn")
    phone.click(".raid-list .rl-tool:has-text('Attendance')")
    phone.wait_for_selector(".attendance-table-dialog .att-table tbody tr", timeout=10000)
    phone.screenshot(path=f"{SHOTS}/54-phone-attendance-table.png")
    phone.click(".attendance-table-dialog button[title='Close']")
    scroll_to_card(phone, "Molten Core run")
    phone.click(".event-card:has-text('Molten Core run') .ev-btn.attendance")
    phone.wait_for_selector(".attendance-dialog .att-row")
    phone.screenshot(path=f"{SHOTS}/55-phone-take-attendance.png")
    phone.click(".attendance-dialog button[title='Close']")
    phone.click("#nav-toggle")
    phone.wait_for_timeout(400)
    phone.click("#home-btn")
    phone.wait_for_selector("#dm-list .dm-row:has-text('Dave')", timeout=10000)
    phone.click("#dm-list .dm-row:has-text('Dave')")
    phone.wait_for_selector("#messages article.msg:has-text('Bring snacks')", timeout=10000)
    phone.click("#emoji-btn")
    phone.click(".emoji-tabs .custom-tab")
    phone.wait_for_selector(".custom-pane button[title=':pog:']")
    phone.screenshot(path=f"{SHOTS}/33-phone-emoji-picker.png")
    phone.keyboard.press("Escape")

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
    droid.keyboard.press("Escape")
    droid.click("#nav-toggle")
    droid.wait_for_timeout(400)
    droid.click("#home-btn")
    droid.wait_for_selector("#dm-list .dm-row", timeout=10000)
    droid.wait_for_timeout(300)
    droid.screenshot(path=f"{SHOTS}/34-android-dms.png")
    if not droid.evaluate("document.getElementById('app').classList.contains('show-nav')"):
        droid.click("#nav-toggle")
        droid.wait_for_timeout(400)
    droid.click(".guild[title='UI Test Guild']")
    droid.wait_for_timeout(300)
    if droid.evaluate("document.getElementById('app').classList.contains('show-nav')"):
        droid.click(".channel:has-text('raids')")
    droid.wait_for_selector(".event-card", timeout=10000)
    droid.click("#raid-btn")
    droid.click(".raid-list .rl-tool:has-text('Calendar')")
    droid.wait_for_selector(".calendar-dialog .cal-agenda", timeout=10000)
    droid.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    droid.wait_for_timeout(300)
    droid.screenshot(path=f"{SHOTS}/56-android-calendar.png")
    droid.click(".calendar-dialog button[title='Close']")
    droid.click("#raid-btn")
    droid.click(".raid-list button.primary")
    droid.wait_for_selector(".raid-dialog[open]")
    droid.select_option(".raid-dialog select[aria-label='Repeat']", "2")
    droid.wait_for_timeout(200)
    droid.screenshot(path=f"{SHOTS}/57-android-plan-repeating.png")
    droid.keyboard.press("Escape")

    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("UI test passed.")
