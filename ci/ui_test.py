"""Clicks through the web client in headless Chrome against a real server.

Fails on any JavaScript error and saves screenshots (uploaded by CI) so the
new screens can be eyeballed without deploying.
"""

import json
import os
import sys
import time
import urllib.error
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
carol = api("POST", "/api/auth/register", {"username": "carolui", "password": "password123", "display_name": "Carol"})
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

# ---- Batch 7: sign-up posting times, games ----
# A raid in 10 days whose sign-up post goes up 3 days before (so it's only in the calendar for now).
pending_start = iso((time.time() // 86400 + 10) * 86400 + 20 * 3600)
pending = api("POST", f"/api/channels/{chan['id']}/events", {
    "title": "Naxxramas prep", "starts_at": pending_start, "game": "forever",
    "post_rule": {"mode": "days", "days": 3, "time_zone": "UTC"},
}, TOKEN)
assert pending["event"]["message_id"] is None and pending["event"]["signups_open_at"], pending

# ---- Batch 4: WoW tools (Wowhead and Battle.net are the mock in ci/wow_mock.py) ----
lfgc = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "lfg"}, TOKEN)
api("POST", "/api/users/@me/characters", {"name": "Lightwell", "last_name": "Dawnstar", "ruleset": "PvP", "race": "Human", "class": "Priest", "level": 60,
    "professions": ["Tailoring", "Enchanting"], "armory_url": "https://example.com/lightwell", "is_main": True}, TOKEN)
api("POST", "/api/users/@me/characters", {"name": "Bubbles", "last_name": "Hearthglen", "class": "Paladin", "level": 58,
    "professions": ["Mining", "Blacksmithing"]}, TOKEN)
api("POST", "/api/users/@me/characters", {"game": "tbc", "name": "Gemcutter", "realm": "Spineshatter", "race": "Blood Elf", "class": "Paladin", "level": 70,
    "professions": ["Jewelcrafting", "Mining"]}, TOKEN)
api("POST", "/api/users/@me/characters", {"name": "Shieldwall", "last_name": "Ironfoe", "class": "Warrior", "level": 60, "professions": ["Mining"]}, dave["token"])
api("POST", "/api/users/@me/characters", {"name": "Sparkles", "last_name": "Fizzwick", "class": "Mage", "level": 59}, erin["token"])
api("POST", f"/api/channels/{lfgc['id']}/messages", {"content": "Anyone keen for dungeons tonight?"}, TOKEN)
scholo = api("POST", f"/api/channels/{lfgc['id']}/lfg", {"activity": "Scholomance", "note": "Got the key. Need a healer, then we go.",
    "game": "forever", "leader": {"role": "tank", "class": "Warrior", "character": "Shieldwall Ironfoe", "level": 60}}, dave["token"])
api("PUT", f"/api/channels/{lfgc['id']}/lfg/{scholo['id']}/join", {"role": "dps", "class": "Mage", "character": "Sparkles Fizzwick", "level": 59}, erin["token"])
gold = api("POST", f"/api/channels/{lfgc['id']}/lfg", {"activity": "Gold farming in Searing Gorge", "max_tanks": 0, "max_healers": 0, "max_dps": 2,
    "starts_at": iso(time.time() + 3 * 3600), "leader": {"role": "dps", "class": "Mage", "character": "Sparkles", "level": 59}}, erin["token"])
api("PUT", f"/api/channels/{lfgc['id']}/lfg/{gold['id']}/join", {"role": "dps", "class": "Warrior", "character": "Shieldwall", "level": 60}, dave["token"])
api("POST", f"/api/channels/{lfgc['id']}/wow/item", {"query": "thunderfury"}, dave["token"])
api("POST", f"/api/channels/{lfgc['id']}/wow/item", {"query": "not a real item"}, dave["token"])
api("POST", f"/api/channels/{lfgc['id']}/messages", {"content": "Logs from last night https://classic.warcraftlogs.com/reports/aBcD1234 "
    "and my BiS https://www.wowhead.com/classic/item=16922/leggings-of-transcendence https://raider.io/characters/us/forever/Sparkles"}, erin["token"])
api("POST", f"/api/channels/{lfgc['id']}/wow/char", {"query": "Shieldwall"}, erin["token"])

# ---- Batch 5: voice extras (moderation, entrance sounds, soundboard favourites, screen share) ----
def make_wav(seconds, rate=8000):
    n = int(rate * seconds)
    # A quiet two-tone beep so it's audibly a sound if anyone listens.
    import math
    data = bytes(int(128 + 40 * math.sin(2 * math.pi * (440 if i < n // 2 else 660) * i / rate)) for i in range(n))
    return (b"RIFF" + struct.pack("<I", 36 + n) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate, 1, 8)
            + b"data" + struct.pack("<I", n) + data)


frank = api("POST", "/api/auth/register", {"username": f"frank{suffix}", "password": "password123", "display_name": "Frank"})
api("POST", f"/api/invites/{code}", None, frank["token"])
FRANK = frank["user"]["id"]
comms = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "Raid comms", "kind": 2}, TOKEN)
afk = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "AFK", "kind": 2}, TOKEN)
upload("/api/users/@me/entrance-sound", {}, {"file": ("hello.wav", make_wav(1), "audio/wav")}, dave["token"])
for name, emoji in [("Leeroy", "🐔"), ("Bonk", "🔨"), ("Wipe", "💀"), ("Bloodlust", "🥁"), ("Ding", "✨")]:
    snd = upload(f"/api/guilds/{guild['id']}/sounds", {"name": name, "emoji": emoji}, {"file": (f"{name}.wav", make_wav(0.5), "audio/wav")}, TOKEN)
    if name in ("Leeroy", "Wipe"):
        api("PUT", f"/api/sounds/{snd['id']}/favourite", None, TOKEN)
with open("/tmp/entrance-test.wav", "wb") as f:
    f.write(make_wav(1.5))

def scroll_to_card(pg, title):
    """Scrolls a raid card into view (by title; cards get redrawn, so no element handles)."""
    pg.evaluate("t => [...document.querySelectorAll('#messages .event-card')].find((c) => c.querySelector('.ev-title')?.textContent.includes(t))?.scrollIntoView({block: 'center'})", title)
    pg.wait_for_timeout(250)


errors = []

# Every screenshot also checks nothing on screen reads just "null" or "undefined"
# (a `cond ? node : null` that ended up as text).
from playwright.sync_api import Page

STRAY_JS = """() => {
  const out = [];
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const t = n.nodeValue.trim();
    if (t !== "null" && t !== "undefined" && t !== "false") continue;
    const p = n.parentElement;
    if (!p || !p.getClientRects().length || getComputedStyle(p).visibility === "hidden") continue;
    out.push(`${t} in <${p.tagName.toLowerCase()} class="${p.className}">`);
  }
  return out;
}"""
_page_screenshot = Page.screenshot


def _checked_screenshot(self, *args, **kwargs):
    try:
        stray = self.evaluate(STRAY_JS)
    except Exception:
        stray = []
    if stray:
        errors.append(f"stray text on {os.path.basename(kwargs.get('path', '?'))}: {stray}")
    return _page_screenshot(self, *args, **kwargs)


Page.screenshot = _checked_screenshot


def check_settings_tabs(pg, opener, label):
    """Clicks through every tab of a settings window, forwards then backwards, and checks
    each time that only that page is showing (nothing left over from the one before)."""
    pg.wait_for_selector("dialog.settings", state="detached")
    pg.evaluate(opener)
    pg.wait_for_selector("dialog.settings .settings-nav button")
    pg.wait_for_timeout(800)
    tabs = pg.evaluate("""[...document.querySelectorAll('dialog.settings .settings-nav button')]
        .map((b) => b.textContent.trim()).filter((t) => t && !t.startsWith('＋') && t !== 'Sign out')""")
    roles = {r["name"] for r in api("GET", f"/api/guilds/{G}/roles", None, TOKEN)}
    heads = {}
    for n, order in enumerate([tabs, list(reversed(tabs))]):
        for t in order:
            pg.click(f"dialog.settings .settings-nav button:text-is({json.dumps(t)})")
            pg.wait_for_function("document.querySelector('dialog.settings .settings-main h2')", timeout=10000)
            info, last = None, None
            for _ in range(25):  # until the page has finished drawing
                pg.wait_for_timeout(400)
                info = pg.evaluate("""() => { const m = document.querySelector('dialog.settings .settings-main');
                    return { pages: m.children.length, heads: [...m.querySelectorAll('h2')].map((h) => h.textContent.trim()) }; }""")
                if info == last:
                    break
                last = info
            assert info["pages"] == 1, f"{label}/{t}: {info['pages']} pages showing"
            assert len(set(info["heads"])) == len(info["heads"]), f"{label}/{t}: page shown twice: {info['heads']}"
            if t in heads:
                assert info["heads"] == heads[t], f"{label}/{t}: {info['heads']} now, {heads[t]} the first time"
            heads[t] = info["heads"]
            if n == 0 and t in ("Bots", "Emoji", "Badges", "Audit log", "Profile & picture", "Webhooks & news feeds"):
                pg.screenshot(path=f"{SHOTS}/120-{label}-{''.join(ch for ch in t.lower() if ch.isalnum())[:14]}.png")
    pages = {t: h for t, h in heads.items() if t not in roles and h}
    for a, ha in pages.items():
        for b, hb in pages.items():
            assert a == b or ha[0] not in hb, f"{label}: {a}'s heading {ha[0]!r} showed on {b}: {hb}"
    # Two quick clicks: only the second page ends up showing.
    if "Bots" in tabs and "Emoji" in tabs:
        pg.click("dialog.settings .settings-nav button:text-is('Bots')")
        pg.click("dialog.settings .settings-nav button:text-is('Emoji')")
        pg.wait_for_timeout(1200)
        got = pg.evaluate("[...document.querySelectorAll('dialog.settings .settings-main h2')].map((h) => h.textContent.trim())")
        assert got == heads["Emoji"], f"{label}: after Bots then Emoji: {got}"
    print(f"settings tabs ({label}):", {t: h[:1] for t, h in heads.items()})
    pg.keyboard.press("Escape")
    pg.wait_for_selector("dialog.settings", state="detached")


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


with sync_playwright() as p:
    # Fake microphone and screen for the voice tests (LiveKit runs in Docker in CI).
    browser = p.chromium.launch(args=[
        "--use-fake-ui-for-media-stream",
        "--use-fake-device-for-media-stream",
        "--autoplay-policy=no-user-gesture-required",
    ])

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
    page.wait_for_timeout(800)  # let the channel finish settling at the bottom first
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
    # Raids whose sign-ups open later show as such; planning from the calendar asks where to post.
    page.evaluate(f"openCalendar(new Date({json.dumps(pending_start)}))")
    page.wait_for_selector(".calendar-dialog .cal-tz")
    page.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    page.wait_for_selector(".calendar-dialog .cal-chip.pending:has-text('Naxxramas prep')", timeout=10000)
    page.wait_for_selector(".calendar-dialog .cal-daypanel .event-card.not-open .ev-opens:has-text('Sign-ups open')")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/113-calendar-signups-open-later.png")
    page.click(".calendar-dialog .cal-plan-day")
    page.wait_for_selector(".raid-dialog[open] select[aria-label='Post in']")
    opts = page.evaluate("[...document.querySelectorAll(\".raid-dialog select[aria-label='Post in'] option\")].map((o) => o.textContent)")
    assert "# raids" in opts and "# general" in opts and "# lfg" in opts, opts
    assert page.input_value(".raid-dialog select[aria-label='Post in']") == chan["id"]
    assert page.input_value(".raid-dialog select[aria-label='Post sign-ups']") == "now"
    page.select_option(".raid-dialog select[aria-label='Post in']", general["id"])
    page.fill(".raid-dialog input[maxlength='100']", "AQ40 from the calendar")
    page.select_option(".raid-dialog select[aria-label='Game']", "tbc")
    page.select_option(".raid-dialog select[aria-label='Post sign-ups']", "weekday")
    page.select_option(".raid-dialog select[aria-label='Post on']", "0")
    page.fill(".raid-dialog input[aria-label='Post at']", "09:00")
    page.wait_for_selector(".raid-dialog .post-preview:has-text('goes up')")
    page.screenshot(path=f"{SHOTS}/110-plan-from-calendar.png")
    page.click(".raid-dialog button[type=submit]")
    page.wait_for_selector(".calendar-dialog .cal-daypanel .event-card.not-open:has-text('AQ40 from the calendar') .ev-game:has-text('TBC')", timeout=10000)
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/114-calendar-after-plan.png")
    page.click(".calendar-dialog button[title='Close']")
    page.wait_for_selector(".calendar-dialog", state="detached")
    aq = [e for e in api("GET", f"/api/guilds/{guild['id']}/events", None, TOKEN) if e["title"] == "AQ40 from the calendar"]
    assert len(aq) == 1 and aq[0]["channel_id"] == general["id"] and aq[0]["message_id"] is None and aq[0]["game"] == "tbc", aq
    assert aq[0]["post_rule"]["mode"] == "weekday" and aq[0]["post_rule"]["time"] == "09:00", aq
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

    # ---- WoW tools: group finder, item and link cards, characters ----
    page.click(".channel:has-text('lfg')")
    page.wait_for_selector("#messages .lfg-card:has-text('Scholomance')", timeout=10000)
    page.wait_for_selector("#messages .lfg-card.full:has-text('Gold farming')")
    page.wait_for_selector("#messages .wow-link.site-wcl", timeout=20000)
    page.wait_for_selector("#messages .wow-item .wow-title:has-text('Leggings of Transcendence')", timeout=10000)
    page.wait_for_selector("#messages .wow-char:has-text('Shieldwall')")
    page.wait_for_timeout(300)
    page.evaluate("document.querySelector('#messages .lfg-card').scrollIntoView({block: 'start'})")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/60-lfg-cards.png")
    page.click(".lfg-card:has-text('Scholomance') .lfg-join:has-text('Healer')")
    page.wait_for_selector(".lfg-join-pop .char-chip.on:has-text('Lightwell')")
    page.screenshot(path=f"{SHOTS}/61-lfg-join.png")
    page.click(".lfg-join-pop button.primary")
    page.wait_for_selector(".lfg-card:has-text('Scholomance') .lfg-col.healer .lfg-person.me", timeout=10000)
    page.evaluate("document.querySelector('#messages .wow-item').scrollIntoView({block: 'start'})")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/62-item-cards.png")
    page.evaluate("document.querySelector('#messages .wow-link').scrollIntoView({block: 'start'})")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/63-link-cards.png")
    items_before = page.evaluate("document.querySelectorAll('#messages .wow-item').length")
    page.fill("#composer-input", "/item thunderfury")
    page.press("#composer-input", "Enter")
    page.wait_for_function(f"document.querySelectorAll('#messages .wow-item').length > {items_before}", timeout=15000)
    page.wait_for_selector("#messages .interaction-ref:has-text('/item')")
    page.click("#lfg-btn")
    page.wait_for_selector(".lfg-dialog[open]")
    page.fill(".lfg-dialog input[aria-label='Activity']", "Stratholme: Undead")
    assert page.input_value(".lfg-dialog input[aria-label='Lowest level']") == "58"
    page.select_option(".lfg-dialog select[aria-label='Your role']", "healer")
    page.fill(".lfg-dialog textarea", "Baron run, 45 minute timer")
    page.screenshot(path=f"{SHOTS}/64-lfg-dialog.png")
    page.click(".lfg-dialog button[type=submit]")
    page.wait_for_selector(".lfg-card:has-text('Stratholme: Undead') .lfg-col.healer .lfg-person.me", timeout=10000)
    page.evaluate("openUserSettings('look')")
    page.wait_for_selector(".chars-settings .char-row:has-text('Lightwell')", timeout=10000)
    page.wait_for_selector(".chars-settings .bnet-box .bnet-link")
    page.evaluate("document.querySelector('.chars-settings').scrollIntoView()")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/65-characters-settings.png")
    page.click(".chars-settings .add-char")
    page.wait_for_selector(".char-dialog[open]")
    page.fill(".char-dialog input[aria-label='Name']", "Stabbystab")
    page.select_option(".char-dialog select[aria-label='Class']", "Rogue")
    page.fill(".char-dialog input[aria-label='Level']", "34")
    page.click(".char-dialog .prof-chip:has-text('Skinning')")
    # WoW: Forever needs a last name.
    page.click(".char-dialog button[type=submit]")
    page.wait_for_selector(".char-dialog .error:has-text('last name')")
    page.fill(".char-dialog input[aria-label='Last name']", "Shadowstep")
    page.select_option(".char-dialog select[aria-label='Ruleset']", "Hardcore")
    page.screenshot(path=f"{SHOTS}/66-add-character.png")
    page.click(".char-dialog button[type=submit]")
    page.wait_for_selector(".chars-settings .char-row:has-text('Stabbystab Shadowstep'):has-text('Hardcore')", timeout=10000)
    # TBC: one name and a realm, level 70, Jewelcrafting, Blood Elf / Draenei.
    page.click(".chars-settings .add-char")
    page.wait_for_selector(".char-dialog[open]")
    page.select_option(".char-dialog select[aria-label='Game']", "tbc")
    page.wait_for_selector(".char-dialog input[aria-label='Realm']")
    assert not page.query_selector(".char-dialog input[aria-label='Last name']")
    assert page.get_attribute(".char-dialog input[aria-label='Level']", "max") == "70"
    page.fill(".char-dialog input[aria-label='Name']", "Shinybolt")
    page.fill(".char-dialog input[aria-label='Realm']", "Thunderstrike")
    page.select_option(".char-dialog select[aria-label='Race']", "Draenei")
    page.select_option(".char-dialog select[aria-label='Class']", "Shaman")
    page.fill(".char-dialog input[aria-label='Level']", "70")
    page.click(".char-dialog .prof-chip:has-text('Jewelcrafting')")
    page.screenshot(path=f"{SHOTS}/111-add-character-tbc.png")
    # Back to Forever: the TBC-only profession goes.
    page.select_option(".char-dialog select[aria-label='Game']", "forever")
    assert not page.query_selector(".char-dialog .prof-chip:has-text('Jewelcrafting')")
    page.select_option(".char-dialog select[aria-label='Game']", "tbc")
    page.click(".char-dialog .prof-chip:has-text('Jewelcrafting')")
    page.click(".char-dialog button[type=submit]")
    page.wait_for_selector(".chars-settings .char-row:has-text('Shinybolt'):has-text('Thunderstrike') .char-game:has-text('TBC')", timeout=10000)
    page.evaluate("document.querySelector('.chars-settings').scrollIntoView()")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/112-characters-with-games.png")
    page.keyboard.press("Escape")
    page.wait_for_selector("dialog.settings", state="detached")
    page.click("#messages .msg-author:has-text('Carol') >> nth=0")
    page.wait_for_selector(".profile-card .pc-chars .char-chip:has-text('Lightwell Dawnstar')", timeout=10000)
    page.wait_for_selector(".profile-card .pc-chars .char-chip:has-text('Gemcutter') .char-game:has-text('TBC')")
    page.screenshot(path=f"{SHOTS}/67-profile-characters.png")
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
    # WoW tools on a phone.
    phone.click("#nav-toggle")
    phone.wait_for_timeout(400)
    phone.click(".guild[title='UI Test Guild']")
    phone.wait_for_timeout(400)
    if not phone.evaluate("document.getElementById('app').classList.contains('show-nav')"):
        phone.click("#nav-toggle")
        phone.wait_for_timeout(400)
    phone.click(".channel:has-text('lfg')")
    phone.wait_for_selector("#messages .lfg-card:has-text('Scholomance')", timeout=10000)
    phone.wait_for_timeout(500)
    phone.evaluate("document.querySelector('#messages .lfg-card').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/68-phone-lfg-card.png")
    phone.evaluate("document.querySelector('#messages .wow-item').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/69-phone-item-cards.png")
    phone.evaluate("document.querySelector('#messages .wow-link').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/70-phone-link-cards.png")
    phone.evaluate("[...document.querySelectorAll('#messages .lfg-card')].find((c) => c.textContent.includes('Stratholme'))?.scrollIntoView({block: 'center'})")
    phone.click(".lfg-card:has-text('Stratholme') .lfg-join:has-text('DPS')")
    phone.wait_for_selector(".lfg-join-pop")
    phone.screenshot(path=f"{SHOTS}/71-phone-lfg-join.png")
    phone.keyboard.press("Escape")
    phone.click("#lfg-btn")
    phone.wait_for_selector(".lfg-dialog[open]")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/72-phone-lfg-dialog.png")
    phone.keyboard.press("Escape")
    phone.wait_for_selector(".lfg-dialog", state="detached")
    # Sign-ups that open later, and planning from the calendar (phone).
    phone.evaluate(f"openCalendar(new Date({json.dumps(pending_start)}))")
    phone.wait_for_selector(".calendar-dialog .cal-agenda .cal-opens:has-text('Sign-ups open')", timeout=10000)
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/115-phone-calendar-pending.png")
    phone.click(".calendar-dialog .cal-agenda-row:has-text('Naxxramas prep')")
    phone.wait_for_selector(".calendar-dialog .cal-agenda-item.open .event-card.not-open .ev-opens")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/115b-phone-calendar-pending-open.png")
    phone.click(".calendar-dialog .cal-plan-day")
    phone.wait_for_selector(".raid-dialog[open] select[aria-label='Post in']")
    phone.select_option(".raid-dialog select[aria-label='Repeat']", "1")
    phone.wait_for_selector(".raid-dialog .post-extra:not([hidden]) input[aria-label='Post at']")
    assert phone.input_value(".raid-dialog select[aria-label='Post sign-ups']") == "weekday"
    phone.evaluate("document.querySelector('.raid-dialog .post-field').scrollIntoView({block: 'center'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/116-phone-plan-post-time.png")
    phone.evaluate("document.querySelector('.raid-dialog h2').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/116b-phone-plan-top.png")
    phone.keyboard.press("Escape")
    phone.wait_for_selector(".raid-dialog", state="detached")
    phone.click(".calendar-dialog button[title='Close']")
    phone.wait_for_selector(".calendar-dialog", state="detached")
    # Character form on a phone: WoW: Forever, then TBC.
    phone.evaluate("openUserSettings('look')")
    phone.wait_for_selector(".chars-settings .add-char", timeout=10000)
    phone.evaluate("document.querySelector('.chars-settings').scrollIntoView()")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/117-phone-characters.png")
    phone.click(".chars-settings .add-char")
    phone.wait_for_selector(".char-dialog[open] input[aria-label='Last name']")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/117b-phone-character-forever.png")
    phone.select_option(".char-dialog select[aria-label='Game']", "tbc")
    phone.wait_for_selector(".char-dialog input[aria-label='Realm']")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/118-phone-character-tbc.png")
    phone.keyboard.press("Escape")
    phone.wait_for_selector(".char-dialog", state="detached")
    phone.keyboard.press("Escape")
    phone.wait_for_selector("dialog.settings", state="detached")

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
    droid.evaluate("document.querySelector('.raid-dialog .post-field').scrollIntoView({block: 'center'})")
    droid.wait_for_timeout(200)
    droid.screenshot(path=f"{SHOTS}/119-android-plan-post-time.png")
    droid.keyboard.press("Escape")
    droid.evaluate("openUserSettings('look')")
    droid.wait_for_selector(".chars-settings .char-row", timeout=10000)
    droid.evaluate("document.querySelector('.chars-settings').scrollIntoView()")
    droid.wait_for_timeout(300)
    droid.screenshot(path=f"{SHOTS}/73-android-characters.png")
    droid.click(".chars-settings .add-char")
    droid.wait_for_selector(".char-dialog[open]")
    droid.select_option(".char-dialog select[aria-label='Game']", "tbc")
    droid.wait_for_selector(".char-dialog input[aria-label='Realm']")
    droid.wait_for_timeout(200)
    droid.screenshot(path=f"{SHOTS}/119b-android-character-tbc.png")
    droid.keyboard.press("Escape")
    droid.wait_for_selector(".char-dialog", state="detached")
    droid.keyboard.press("Escape")
    droid.wait_for_selector("dialog.settings", state="detached")
    droid.click("#nav-toggle")
    droid.wait_for_timeout(400)
    droid.click(".channel:has-text('lfg')")
    droid.wait_for_selector("#messages .lfg-card", timeout=10000)
    droid.wait_for_timeout(500)
    droid.evaluate("document.querySelector('#messages .lfg-card').scrollIntoView({block: 'start'})")
    droid.wait_for_timeout(200)
    droid.screenshot(path=f"{SHOTS}/74-android-lfg.png")

    # ---- Voice extras: two people in a real LiveKit room ----
    def voice_page(token, label, viewport=None, mobile=False, android=False):
        pg = browser.new_page(viewport=viewport or {"width": 1280, "height": 800}, is_mobile=mobile, has_touch=mobile,
                              permissions=["microphone", "camera"])
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if android else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
        pg.goto(BASE + "/")
        pg.wait_for_selector("#voice-channels .channel.voice", timeout=15000)
        return pg

    def join(pg, name, drawer=False):
        if drawer and not pg.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            pg.click("#nav-toggle")
            pg.wait_for_timeout(400)
        pg.click(f"#voice-channels .channel.voice:has-text('{name}')")
        pg.wait_for_selector("#voice-panel .vp-status:not(.connecting)", timeout=20000)
        pg.wait_for_function("state.voice.room && state.voice.room.state === 'connected'", timeout=20000)

    carolv = voice_page(TOKEN, "carol-voice")
    join(carolv, "Raid comms")
    davev = voice_page(dave["token"], "dave-voice")
    join(davev, "Raid comms")
    # Dave's entrance sound plays for Carol.
    carolv.wait_for_function(f"state.voice.lastEntrance?.user_id === {json.dumps(DAVE)} && state.voice.lastEntrance.played", timeout=10000)
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}']", timeout=10000)

    # Screen share: Carol shares, Dave sees a tile and a LIVE badge.
    carolv.click("#voice-panel .vp-btn.share")
    carolv.wait_for_selector(".share-picker .share-seg button.on:has-text('1080p')")
    carolv.click(".share-picker .share-seg button:has-text('720p')")
    carolv.click(".share-picker .share-seg button:has-text('30 fps')")
    carolv.wait_for_selector(".share-picker .share-hint:has-text('Smooth motion')")
    carolv.screenshot(path=f"{SHOTS}/79-share-quality-picker.png")
    carolv.click(".share-picker button.share-go")
    carolv.wait_for_selector("#screen-stage .screen-tile.mine", timeout=15000)
    carolv.wait_for_selector("#voice-panel .vp-btn.share.on")
    saved = carolv.evaluate("JSON.parse(localStorage.getItem('outhouse_share_quality'))")
    assert saved["res"] == "720p" and saved["fps"] == 30, saved
    carolv.wait_for_selector("#screen-stage .screen-tile.mine .st-quality:has-text('720p · 30 fps')", timeout=10000)
    davev.wait_for_selector("#screen-stage .screen-tile .st-quality:has-text('720p · 30 fps')", timeout=15000)
    davev.wait_for_selector(f"#voice-channels .voice-user[data-id='{CAROL}'] .live-badge", timeout=10000)
    davev.wait_for_selector(f"#screen-stage .screen-tile[data-id='{CAROL}']", timeout=15000)
    davev.wait_for_function(f"document.querySelector(\"#screen-stage .screen-tile[data-id='{CAROL}'] video\").videoWidth > 0", timeout=15000)
    davev.screenshot(path=f"{SHOTS}/80-screen-share-strip.png")
    davev.click(f"#screen-stage .screen-tile[data-id='{CAROL}']")
    davev.wait_for_selector("#screen-stage.focused .screen-tile.big")
    davev.wait_for_timeout(300)
    davev.screenshot(path=f"{SHOTS}/81-screen-share-focused.png")
    davev.click("#screen-stage .screen-tile.big .st-btn[title='Back to the chat']")
    davev.wait_for_selector("#screen-stage:not(.focused)")

    # A phone watching (no share button needed there, but it can watch).
    erinv = voice_page(erin["token"], "erin-phone", {"width": 390, "height": 844}, mobile=True)
    join(erinv, "Raid comms", drawer=True)
    erinv.wait_for_selector(f"#voice-channels .voice-user[data-id='{CAROL}'] .live-badge", timeout=10000)
    erinv.screenshot(path=f"{SHOTS}/82-phone-voice-panel.png")
    erinv.click(f"#voice-channels .voice-user[data-id='{CAROL}'] .live-badge")
    erinv.wait_for_selector("#screen-stage.focused .screen-tile.big", timeout=15000)
    erinv.wait_for_timeout(400)
    erinv.screenshot(path=f"{SHOTS}/83-phone-screen-share.png")
    erinv.keyboard.press("Escape")

    # The Android app hides Share screen (Android can't capture the screen).
    frankv = voice_page(frank["token"], "frank-android", {"width": 393, "height": 852}, mobile=True, android=True)
    join(frankv, "Raid comms", drawer=True)
    assert not frankv.query_selector("#voice-panel .vp-btn.share"), "share button shown on Android"
    assert carolv.query_selector("#voice-panel .vp-btn.share"), "share button missing on desktop"
    frankv.screenshot(path=f"{SHOTS}/84-android-voice-panel.png")

    # Moderation: right-click Dave in voice.
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.wait_for_selector(".menu button:has-text('Server mute')")
    carolv.wait_for_selector(".menu .menu-slider input[max='200']")
    carolv.screenshot(path=f"{SHOTS}/85-voice-user-menu.png")
    carolv.click(".menu button:has-text('Server mute')")
    davev.wait_for_selector("#voice-panel .vp-btn.forced", timeout=10000)
    davev.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.server", timeout=10000)
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.server", timeout=10000)
    davev.wait_for_function("state.voice.serverMuted === true")
    davev.screenshot(path=f"{SHOTS}/86-server-muted.png")
    # Rejoining keeps the mute.
    davev.click("#voice-panel .icon-btn.danger")
    davev.wait_for_selector("#voice-panel", state="hidden")
    join(davev, "Raid comms")
    davev.wait_for_function("state.voice.serverMuted === true", timeout=10000)
    davev.wait_for_selector("#voice-panel .vp-btn.forced")
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Server mute')")
    davev.wait_for_function("state.voice.serverMuted === false", timeout=10000)
    davev.wait_for_selector("#voice-panel .vp-btn.forced", state="detached")
    # Dave can talk again: his mic gets published.
    davev.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!p && !p.isMuted; })()", timeout=15000)

    # Server deafen, then undeafen: Dave hears Carol again.
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Server deafen')")
    davev.wait_for_function("state.voice.serverDeafened === true", timeout=10000)
    davev.wait_for_selector("#voice-panel .vp-btn.forced")
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.server")
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Server deafen')")
    davev.wait_for_function("state.voice.serverDeafened === false", timeout=10000)
    davev.wait_for_function(f"(() => {{ const p = state.voice.room.remoteParticipants.get({json.dumps(CAROL)}); const pub = p && p.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!pub && pub.isSubscribed && !!pub.track; }})()", timeout=15000)

    # Mute Dave just for Carol.
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Mute for me')")
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.local")
    carolv.screenshot(path=f"{SHOTS}/87-voice-list-desktop.png")

    # Move Dave to AFK, then disconnect him.
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.hover(".menu button:has-text('Move to')")
    carolv.click(".menu button:has-text('AFK')")
    davev.wait_for_selector("#voice-panel .vp-channel:has-text('AFK')", timeout=20000)
    davev.wait_for_function("state.voice.room && state.voice.room.state === 'connected' && state.voice.channelId === " + json.dumps(afk["id"]), timeout=20000)
    carolv.wait_for_function(f"state.voiceStates.get({json.dumps(DAVE)})?.channel_id === {json.dumps(afk['id'])}", timeout=10000)
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Disconnect from voice')")
    davev.wait_for_selector("#voice-panel", state="hidden", timeout=10000)
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}']", state="detached", timeout=10000)

    # Soundboard with favourites.
    carolv.click("#voice-panel .vp-btn[title='Soundboard']")
    carolv.wait_for_selector(".soundboard .sb-label:has-text('Favourites')")
    carolv.screenshot(path=f"{SHOTS}/88-soundboard.png")
    carolv.click(".soundboard .sound-btn:has-text('Bonk') >> nth=0")
    carolv.wait_for_selector(".soundboard.cooling")
    carolv.click(".soundboard .sound-tile:has-text('Ding') .sound-star")
    carolv.wait_for_selector(".soundboard .sound-tile.fav:has-text('Ding')")
    carolv.keyboard.press("Escape")

    # Stop sharing.
    carolv.click("#voice-panel .vp-btn.share")
    carolv.wait_for_selector("#screen-stage", state="detached", timeout=10000)
    erinv.wait_for_selector("#screen-stage", state="detached", timeout=10000)

    # Entrance sound in settings.
    davev.evaluate("openUserSettings('voice')")
    davev.wait_for_selector(".entrance-box button:has-text('Play')", timeout=10000)
    davev.set_input_files(".entrance-box input[type=file]", "/tmp/entrance-test.wav")
    davev.wait_for_selector(".entrance-box button:has-text('1.5 s')", timeout=10000)
    davev.evaluate("document.querySelector('.entrance-box').scrollIntoView()")
    davev.wait_for_timeout(300)
    davev.screenshot(path=f"{SHOTS}/89-entrance-settings.png")
    davev.keyboard.press("Escape")
    frankv.evaluate("openUserSettings('voice')")
    frankv.wait_for_selector(".entrance-box", timeout=10000)
    frankv.evaluate("document.querySelector('.entrance-box').scrollIntoView()")
    frankv.wait_for_timeout(300)
    frankv.screenshot(path=f"{SHOTS}/90-android-entrance-settings.png")

    for pg in (carolv, davev, erinv, frankv):
        pg.evaluate("leaveVoice(true)")
    carolv.wait_for_timeout(500)
    for pg in (carolv, davev, erinv, frankv):
        pg.close()

    # ---- Batch 6: audit log, badges, server status, notifications, animated profiles ----
    G = guild["id"]
    officer = api("POST", f"/api/guilds/{G}/roles", {"name": "Officer", "color": 0x3FC7EB}, TOKEN)
    api("PUT", f"/api/guilds/{G}/members/{DAVE}/roles/{officer['id']}", None, TOKEN)
    api("PATCH", f"/api/guilds/{G}/roles/{officer['id']}", {"name": "Raid officer"}, TOKEN)
    api("POST", f"/api/guilds/{G}/members/{ERIN}/timeout", {"minutes": 10}, TOKEN)
    api("POST", f"/api/guilds/{G}/members/{ERIN}/timeout", {"minutes": 0}, TOKEN)
    mvp = api("POST", f"/api/guilds/{G}/badges", {"name": "Raid MVP", "emoji": "🏆", "color": 0xFFB347, "description": "Carried the Rag kill"}, TOKEN)
    api("PUT", f"/api/badges/{mvp['id']}/members/{DAVE}", None, TOKEN)
    api("POST", f"/api/guilds/{G}/badges", {"name": "Bard", "emoji": "🎻", "color": 0x8B7CF6, "description": "Sings in voice"}, TOKEN)
    api("PATCH", "/api/users/@me", {"profile_theme": "emberdrift"}, TOKEN)
    api("PATCH", "/api/users/@me", {"profile_theme": "aurora", "about_me": "Tanks for gold."}, dave["token"])
    # A push device for Carol, as if her phone's Chrome had subscribed.
    req = urllib.request.Request(BASE + "/api/push/subscriptions", method="POST", data=json.dumps({
        "endpoint": "https://fcm.googleapis.com/fcm/send/ui-test-device",
        "keys": {"p256dh": "BDrOP4Oow2mdZi3HAPe3hBnzB-hAZb_blV8BZe_vu5eL6i4bYjuyRWOzQIbOobvaIg9vGTBrrkhelt3K2KaB5OU", "auth": "Hu5qIhT2Wlcci3eHdsOA_g"},
    }).encode())
    req.add_header("Content-Type", "application/json")
    req.add_header("Authorization", f"Bearer {TOKEN}")
    req.add_header("User-Agent", "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36")
    urllib.request.urlopen(req).read()

    def fresh(token, label, **kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if label.startswith("android") else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(G)}); localStorage.setItem('outhouse_channel_{G}', {json.dumps(chan['id'])});")
        pg.goto(BASE + "/")
        pg.wait_for_selector(".event-card", timeout=15000)
        return pg

    b6 = fresh(TOKEN, "batch6-desktop", viewport={"width": 1280, "height": 800}, permissions=["notifications"])
    # Badges next to names in the member list (Dave has Raid MVP).
    b6.wait_for_selector(f"#member-list .member[data-id='{DAVE}'] .name-badges", timeout=10000)
    # Audit log.
    b6.evaluate("openSettings('audit')")
    b6.wait_for_selector(".audit-row", timeout=10000)
    b6.wait_for_selector(".audit-row:has-text('Raid officer')")
    b6.wait_for_selector(".audit-row:has-text('Name: Officer → Raid officer')")
    b6.screenshot(path=f"{SHOTS}/91-audit-log.png")
    b6.select_option(".audit-filters select[aria-label='Filter by action']", "member_timeout")
    b6.wait_for_function("document.querySelectorAll('.audit-row').length === 1")
    b6.wait_for_selector(".audit-row:has-text('Erin'):has-text('For 10 minutes')")
    b6.select_option(".audit-filters select[aria-label='Filter by action']", "")
    b6.select_option(".audit-filters select[aria-label='Filter by person']", DAVE)
    b6.wait_for_selector(".audit-row:has-text('Raid MVP')", timeout=10000)
    b6.screenshot(path=f"{SHOTS}/92-audit-filtered.png")
    # Badges: give Bard to Erin from the settings page.
    b6.click(".settings-nav button:has-text('Badges')")
    b6.wait_for_selector(".badge-item:has-text('Bard')", timeout=10000)
    b6.select_option(".badge-item:has-text('Bard') select.give-select", ERIN)
    b6.wait_for_selector(".badge-item:has-text('Bard') .holder-chip:has-text('Erin')", timeout=10000)
    b6.screenshot(path=f"{SHOTS}/93-badge-settings.png")
    b6.click(".settings-nav button:has-text('Server status')")
    b6.wait_for_selector(".status-grid .status-item", timeout=10000)
    b6.screenshot(path=f"{SHOTS}/94-server-status.png")
    b6.keyboard.press("Escape")
    b6.wait_for_selector("dialog.settings", state="detached")
    # Profile cards: animated backgrounds and badges.
    b6.click(f"#member-list .member[data-id='{DAVE}'] .avatar-wrap")
    b6.wait_for_selector(".profile-card.anim-aurora .pc-badges .badge-chip:has-text('Raid MVP')", timeout=10000)
    b6.wait_for_timeout(400)
    b6.screenshot(path=f"{SHOTS}/95-profile-badges-aurora.png")
    b6.keyboard.press("Escape")
    b6.click("#messages .msg-author:has-text('Carol') >> nth=0")
    b6.wait_for_selector(".profile-card.anim-emberdrift .pc-badges .badge-chip:has-text('owner')", timeout=10000)
    b6.wait_for_timeout(400)
    b6.screenshot(path=f"{SHOTS}/96-profile-emberdrift.png")
    b6.keyboard.press("Escape")
    # Mute a channel and set the outhouse's level from right-click menus.
    b6.click("#text-channels .channel:has-text('general')", button="right")
    b6.click(".menu button:has-text('Mute channel')")
    b6.wait_for_selector("#text-channels .channel.muted-channel:has-text('general')", timeout=10000)
    b6.click(f".guild[title='UI Test Guild']", button="right")
    b6.hover(".menu button:has-text('Notifications')")
    b6.wait_for_selector(".menu button:has-text('Only @mentions and replies')")
    b6.screenshot(path=f"{SHOTS}/97-guild-notify-menu.png")
    b6.click(".menu button:has-text('All messages')")
    b6.wait_for_timeout(500)
    st6 = api("GET", "/api/users/@me/notifications", None, TOKEN)
    assert any(l["guild_id"] == G and l["level"] == "all" for l in st6["levels"]), st6
    assert general["id"] in st6["muted_channels"], st6
    # Settings → Notifications.
    b6.evaluate("openUserSettings('notifications')")
    b6.wait_for_selector(".notify-box .device-row:has-text('Chrome on Android')", timeout=10000)
    b6.wait_for_selector(".notify-box .device-row:has-text('#general')")
    b6.wait_for_selector(".notify-box .push-state")
    print("push state in headless Chrome:", b6.evaluate("document.querySelector('.push-state').textContent + ' / permission=' + Notification.permission"))
    b6.screenshot(path=f"{SHOTS}/98-notifications-settings.png")
    b6.select_option(".level-row:has-text('Family') select", "nothing")
    b6.wait_for_timeout(400)
    assert any(l["guild_id"] == family["id"] and l["level"] == "nothing" for l in api("GET", "/api/users/@me/notifications", None, TOKEN)["levels"])
    b6.click(".device-row:has-text('#general') button:has-text('Unmute')")
    b6.wait_for_function("![...document.querySelectorAll('.notify-box')].some((b) => b.textContent.includes('#general'))", timeout=10000)
    # Animated backgrounds in the profile settings.
    b6.click(".settings-nav button:has-text('Profile & picture')")
    b6.wait_for_selector(".theme-swatch.anim-starfield")
    b6.click(".theme-swatch[title='Starfield']")
    b6.wait_for_selector(".pl-preview .profile-card.anim-starfield")
    b6.wait_for_selector(".check-row:has-text('badges next to')")
    b6.wait_for_timeout(300)
    b6.screenshot(path=f"{SHOTS}/99-animated-themes.png")
    b6.keyboard.press("Escape")
    # Reduced motion: no animation.
    # Every settings tab shows only its own page (desktop).
    check_settings_tabs(b6, "openUserSettings('account')", "user")
    check_settings_tabs(b6, "openSettings('roles')", "outhouse")
    b6.emulate_media(reduced_motion="reduce")
    b6.click("#messages .msg-author:has-text('Carol') >> nth=0")
    b6.wait_for_selector(".profile-card.anim-emberdrift")
    anim = b6.evaluate("getComputedStyle(document.querySelector('.profile-card .pc-fx')).animationName")
    assert anim == "none", f"animation still on with reduced motion: {anim}"
    b6.keyboard.press("Escape")
    b6.close()

    # Phone.
    ph6 = fresh(TOKEN, "batch6-phone", viewport={"width": 390, "height": 844}, is_mobile=True, has_touch=True, permissions=["notifications"])
    check_settings_tabs(ph6, "openSettings('roles')", "phone-outhouse")
    ph6.evaluate("openSettings('audit')")
    ph6.wait_for_selector(".audit-row", timeout=10000)
    ph6.wait_for_timeout(300)
    ph6.screenshot(path=f"{SHOTS}/100-phone-audit-log.png")
    ph6.evaluate("document.querySelector('.settings-nav button.active') && [...document.querySelectorAll('.settings-nav button')].find((b) => b.textContent === 'Badges').click()")
    ph6.wait_for_selector(".badge-item", timeout=10000)
    ph6.wait_for_timeout(300)
    ph6.screenshot(path=f"{SHOTS}/101-phone-badges.png")
    ph6.keyboard.press("Escape")
    ph6.wait_for_selector("dialog.settings", state="detached")
    ph6.evaluate("openUserSettings('notifications')")
    ph6.wait_for_selector(".notify-box .level-row", timeout=10000)
    ph6.wait_for_timeout(300)
    ph6.screenshot(path=f"{SHOTS}/102-phone-notifications.png")
    ph6.keyboard.press("Escape")
    ph6.wait_for_selector("dialog.settings", state="detached")
    ph6.click("#messages .msg-author:has-text('Carol') >> nth=0")
    ph6.wait_for_selector(".profile-card .pc-badges", timeout=10000)
    ph6.wait_for_timeout(300)
    ph6.screenshot(path=f"{SHOTS}/103-phone-profile.png")
    ph6.close()

    # Android app.
    dr6 = fresh(TOKEN, "android-batch6", viewport={"width": 393, "height": 852}, is_mobile=True, has_touch=True)
    dr6.evaluate("openUserSettings('notifications')")
    dr6.wait_for_selector(".notify-box:has-text('Android app')", timeout=10000)
    dr6.wait_for_timeout(300)
    dr6.screenshot(path=f"{SHOTS}/104-android-notifications.png")
    dr6.keyboard.press("Escape")
    dr6.wait_for_selector("dialog.settings", state="detached")
    dr6.evaluate("openSettings('status')")
    dr6.wait_for_selector(".status-grid .status-item", timeout=10000)
    dr6.wait_for_timeout(300)
    dr6.screenshot(path=f"{SHOTS}/105-android-server-status.png")
    dr6.close()

    # ---- Raid managers by role, characters on sign-ups, invite lists ----
    raid_leader = api("POST", f"/api/guilds/{G}/roles", {"name": "Raid leader", "color": 0x4F7FD0}, TOKEN)
    api("PUT", f"/api/guilds/{G}/members/{DAVE}/roles/{raid_leader['id']}", None, TOKEN)
    gus = api("POST", "/api/auth/register", {"username": f"gus{suffix}", "password": "password123", "display_name": "Gus"})
    hank = api("POST", "/api/auth/register", {"username": f"hank{suffix}", "password": "password123", "display_name": "Hank"})
    ivy = api("POST", "/api/auth/register", {"username": f"ivy{suffix}", "password": "password123", "display_name": "Ivy"})
    for u in (gus, hank, ivy):
        api("POST", f"/api/invites/{code}", None, u["token"])
    hammer = api("POST", "/api/users/@me/characters", {"name": "Hammer", "last_name": "Stonefist", "race": "Dwarf", "class": "Paladin", "level": 60}, hank["token"])
    api("POST", "/api/users/@me/characters", {"name": "Windy", "last_name": "Skysong", "race": "Skyborne (Horde)", "class": "Shaman", "level": 60}, ivy["token"])
    shieldwall = next(c for c in api("GET", "/api/users/@me/characters", None, dave["token"]) if c["name"] == "Shieldwall")
    sparkles = next(c for c in api("GET", "/api/users/@me/characters", None, erin["token"]) if c["name"] == "Sparkles")
    naxx = api("POST", f"/api/channels/{chan['id']}/events", {
        "title": "Naxx invite test", "description": "Roster check for the invite list.", "starts_at": iso(time.time() + 4 * 86400),
        "game": "forever", "max_tanks": 2, "max_healers": 3, "max_dps": 5,
    }, TOKEN)
    NX = naxx["event"]["id"]
    api("PUT", f"/api/events/{NX}/signup", {"status": "yes", "role": "tank", "spec": "Protection", "character_id": shieldwall["id"]}, dave["token"])
    api("PUT", f"/api/events/{NX}/signup", {"status": "yes", "role": "dps", "spec": "Frost", "character_id": sparkles["id"]}, erin["token"])
    api("PUT", f"/api/events/{NX}/signup", {"status": "late", "role": "dps", "class": "Rogue", "spec": "Combat"}, gus["token"])
    naxx_card = ".event-card:has-text('Naxx invite test')"

    def open_card(pg):
        pg.wait_for_selector(naxx_card, timeout=15000)
        scroll_to_card(pg, "Naxx invite test")

    # Outhouse settings → Raids: Raid leader manages raids; #raids is the sign-up channel.
    rm = fresh(TOKEN, "raids-desktop", viewport={"width": 1280, "height": 800})
    rm.evaluate("openSettings('raids')")
    rm.wait_for_selector(".rs-roles .rs-role:has-text('Raid leader')", timeout=10000)
    rm.click(".rs-role:has-text('Raid leader')")
    rm.select_option("select[aria-label='Sign-up channel']", chan["id"])
    rm.wait_for_selector(".rs-who:has-text('Dave')")
    rm.wait_for_timeout(200)
    rm.screenshot(path=f"{SHOTS}/130-raid-settings.png")
    rm.click(".settings-actions button.primary:has-text('Save changes')")
    for _ in range(30):
        rs = api("GET", f"/api/guilds/{G}/raid-settings", None, dave["token"])
        if rs["manager_role_ids"] == [raid_leader["id"]]:
            break
        rm.wait_for_timeout(200)
    assert rs == {"manager_role_ids": [raid_leader["id"]], "default_channel_id": chan["id"]}, rs
    rm.keyboard.press("Escape")
    rm.wait_for_selector("dialog.settings", state="detached")
    # Plan a raid starts on the sign-up channel even from another channel.
    rm.click(".channel:has-text('general')")
    rm.wait_for_timeout(500)
    rm.evaluate("openRaidDialog()")
    rm.wait_for_selector(".raid-dialog[open] select[aria-label='Post in']")
    assert rm.input_value(".raid-dialog select[aria-label='Post in']") == chan["id"]
    rm.keyboard.press("Escape")
    rm.click(".channel:has-text('raids')")
    # Carol signs up with her main (picked for her); the roster shows the character.
    open_card(rm)
    rm.click(f"{naxx_card} .ev-btn:has-text('Sign up')")
    rm.wait_for_selector(".signup-pop .char-chip.on:has-text('Lightwell')")
    rm.wait_for_selector(".signup-pop .su-char-hint:has-text('Lightwell Dawnstar')")
    rm.click(".signup-pop .su-spec:has-text('Holy') >> nth=0")
    rm.screenshot(path=f"{SHOTS}/131-signup-with-character.png")
    rm.click(".signup-pop button.primary")
    rm.wait_for_selector(f"{naxx_card} .ev-col.healer .su-name.me:has-text('Lightwell Dawnstar')", timeout=10000)
    rm.wait_for_selector(f"{naxx_card} .ev-col.tank .su-name:has-text('Shieldwall Ironfoe')")
    rm.wait_for_selector(f"{naxx_card} .ev-col.tank .ev-owner:has-text('Dave')")
    rm.wait_for_selector(f"{naxx_card} .ev-col.dps .su-name:has-text('Gus')")
    scroll_to_card(rm, "Naxx invite test")
    rm.screenshot(path=f"{SHOTS}/132-roster-characters.png")
    # Export / invite list.
    rm.click(f"{naxx_card} .ev-btn.export")
    rm.wait_for_selector(".export-dialog .exp-text")
    assert rm.inner_text(".export-dialog .exp-tab.on") == "Method Raid Tools (Invite by list)"
    names = rm.input_value(".export-dialog .exp-text")
    assert names == "Shieldwall Ironfoe\nLightwell Dawnstar\nSparkles Fizzwick\nGus", names
    rm.wait_for_selector(".export-dialog .exp-flag:has-text('Gus')")
    rm.wait_for_selector(".export-dialog .exp-summary:has-text('4 names')")
    rm.screenshot(path=f"{SHOTS}/133-export-dialog.png")
    rm.click(".export-dialog .exp-tab:has-text('Comma')")
    assert rm.input_value(".export-dialog .exp-text") == "Shieldwall Ironfoe,Lightwell Dawnstar,Sparkles Fizzwick,Gus"
    rm.click(".export-dialog .exp-tab:has-text('MRT')")
    mrt = rm.input_value(".export-dialog .exp-text")
    assert mrt == "Group 1\nShieldwall Ironfoe\nLightwell Dawnstar\nSparkles Fizzwick\nGus", mrt
    rm.click(".export-dialog .exp-tab:has-text('/invite')")
    macro = rm.input_value(".export-dialog .exp-text")
    assert macro == "/invite Shieldwall Ironfoe\n/invite Lightwell Dawnstar\n/invite Sparkles Fizzwick\n/invite Gus", macro
    rm.uncheck(".export-dialog .check-row:has-text('Tentative') input")
    assert "Gus" not in rm.input_value(".export-dialog .exp-text")
    rm.check(".export-dialog .check-row:has-text('Tentative') input")
    rm.wait_for_timeout(200)
    rm.screenshot(path=f"{SHOTS}/134-export-macros.png")
    rm.keyboard.press("Escape")
    rm.wait_for_selector(".export-dialog", state="detached")
    rm.close()

    # Dave has Raid leader: he runs Carol's raid. He adds Hank (his character is picked) to the bench.
    dv = fresh(dave["token"], "raids-dave", viewport={"width": 1280, "height": 800})
    open_card(dv)
    dv.wait_for_selector(f"{naxx_card} button[title='Raid options']", timeout=10000)
    dv.wait_for_selector(f"{naxx_card} .ev-btn.export")
    dv.click(f"{naxx_card} button[title='Raid options']")
    dv.click(".menu button:has-text('Add someone')")
    dv.wait_for_selector(".add-raid-dialog select[aria-label='Who']")
    dv.select_option(".add-raid-dialog select[aria-label='Who']", hank["user"]["id"])
    dv.wait_for_selector(".add-raid-dialog .char-chip.on:has-text('Hammer Stonefist')", timeout=10000)
    dv.click(".add-raid-dialog .su-spec:has-text('Holy')")
    dv.select_option(".add-raid-dialog select[aria-label='Where']", "bench")
    dv.wait_for_timeout(200)
    dv.screenshot(path=f"{SHOTS}/135-add-someone.png")
    dv.click(".add-raid-dialog button.primary")
    dv.wait_for_selector(f"{naxx_card} .ev-bench .su-name:has-text('Hammer Stonefist')", timeout=10000)
    ex = api("GET", f"/api/events/{NX}/export", None, dave["token"])
    hank_entry = next(e for e in ex["entries"] if e["user_id"] == hank["user"]["id"])
    assert hank_entry["status"] == "bench" and hank_entry["character"]["id"] == hammer["id"], hank_entry
    # Change Gus's role from the roster menu (he has no characters).
    dv.click(f"{naxx_card} .ev-col.dps .su-name:has-text('Gus')")
    dv.click(".menu button:has-text('Change character or role')")
    dv.wait_for_selector(".add-raid-dialog .char-chips:has-text('No WoW: Forever characters')")
    dv.keyboard.press("Escape")
    dv.wait_for_selector(".add-raid-dialog", state="detached")
    scroll_to_card(dv, "Naxx invite test")
    dv.screenshot(path=f"{SHOTS}/136-manager-view.png")
    # Ivy (no raid role) can't run it.
    dv.close()
    iv = fresh(ivy["token"], "raids-ivy", viewport={"width": 1280, "height": 800})
    open_card(iv)
    assert not iv.query_selector(f"{naxx_card} button[title='Raid options']"), "Ivy got raid options"
    assert not iv.query_selector(f"{naxx_card} .ev-btn.export"), "Ivy got the invite list"
    # A Skyborne character in the form, with the Forever combo note.
    iv.evaluate("openUserSettings('look')")
    iv.wait_for_selector(".chars-settings .char-row:has-text('Windy Skysong')", timeout=10000)
    iv.click(".chars-settings .char-row:has-text('Windy Skysong') button:has-text('Edit')")
    iv.wait_for_selector(".char-dialog[open]")
    assert iv.input_value(".char-dialog select[aria-label='Race']") == "Skyborne (Horde)"
    iv.select_option(".char-dialog select[aria-label='Race']", "Skyborne (Alliance)")
    iv.wait_for_selector(".char-dialog .combo-warn:has-text('only Horde Skyborne can be Shamans')")
    iv.screenshot(path=f"{SHOTS}/137-skyborne-character.png")
    iv.keyboard.press("Escape")
    iv.close()

    # Phone and Android app.
    for label, kw, n in (("raids-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 140),
                         ("android-raids", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 150)):
        pg = fresh(TOKEN, label, **kw)
        pg.evaluate("openSettings('raids')")
        pg.wait_for_selector(".rs-roles .rs-role:has-text('Raid leader') input:checked", timeout=10000)
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{n}-{label}-settings.png")
        pg.keyboard.press("Escape")
        pg.wait_for_selector("dialog.settings", state="detached")
        open_card(pg)
        pg.wait_for_selector(f"{naxx_card} .su-name:has-text('Lightwell Dawnstar')")
        pg.screenshot(path=f"{SHOTS}/{n + 1}-{label}-roster.png")
        pg.click(f"{naxx_card} .ev-btn.export")
        pg.wait_for_selector(".export-dialog .exp-text")
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{n + 2}-{label}-export.png")
        pg.click(".export-dialog .exp-tab:has-text('MRT')")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n + 3}-{label}-export-mrt.png")
        pg.keyboard.press("Escape")
        pg.close()


    # ---- Edit channel: topics, slowmode, spoiler and age-restricted channels ----
    api("PATCH", f"/api/channels/{chan['id']}", {
        "topic": f"Raid nights Wed & Sun 8pm server time. Sign up on the cards, ask <@{DAVE}> about loot, strats at https://example.com/strats and `/raid` for a new one."}, TOKEN)
    spoil = api("POST", f"/api/guilds/{G}/channels", {"name": "naxx-spoilers"}, TOKEN)
    api("PATCH", f"/api/channels/{spoil['id']}", {"content_mode": "spoiler", "slowmode_secs": 30,
                                                   "topic": "New raid talk. Everything is blurred until you click it."}, TOKEN)
    for text in ("Kel'Thuzad drops the Frostmourne-looking sword now??", "Phase 3 adds Guardians of Icecrown, save your cooldowns",
                 "Sapphiron has a new frost breath pattern, hide behind the ice blocks"):
        api("POST", f"/api/channels/{spoil['id']}/messages", {"content": text}, dave["token"])
    dark = api("POST", f"/api/guilds/{G}/channels", {"name": "after-dark"}, TOKEN)
    api("PATCH", f"/api/channels/{dark['id']}", {"content_mode": "nsfw", "topic": "Grown-ups only. Keep it legal."}, TOKEN)
    api("PUT", f"/api/channels/{dark['id']}/age-gate", None, TOKEN)
    api("POST", f"/api/channels/{dark['id']}/messages", {"content": "Late night banter lives here."}, TOKEN)
    api("DELETE", f"/api/channels/{dark['id']}/age-gate", None, TOKEN)
    # Dave can't read it until he says he's 18+.
    try:
        api("GET", f"/api/channels/{dark['id']}/messages", None, dave["token"])
        raise AssertionError("18+ channel readable before accepting")
    except urllib.error.HTTPError as e:
        assert e.code == 403, e.code

    ec = fresh(TOKEN, "channels-desktop", viewport={"width": 1280, "height": 800})
    ec.wait_for_selector(".chat-head .topic.has-topic:has-text('Raid nights')", timeout=10000)
    assert ec.evaluate("document.querySelector('.chat-head').textContent.includes('Leave it cleaner')") is False
    ec.screenshot(path=f"{SHOTS}/160-channel-topic-header.png")
    ec.click("#channel-name")
    ec.wait_for_selector(".topic-dialog .topic-full a")
    ec.screenshot(path=f"{SHOTS}/161-topic-dialog.png")
    ec.click(".topic-dialog button:has-text('Edit channel')")
    ec.wait_for_selector("dialog.edit-channel .ec-modes")
    ec.wait_for_timeout(300)
    ec.screenshot(path=f"{SHOTS}/162-edit-channel-overview.png")
    ec.fill("dialog.edit-channel textarea", "Raid nights Wed & Sun 8pm. Bring flasks, <@" + DAVE + "> runs loot.")
    ec.wait_for_selector(".ec-savebar.show")
    ec.screenshot(path=f"{SHOTS}/163-edit-channel-unsaved.png")
    ec.click(".ec-savebar button:has-text('Save changes')")
    ec.wait_for_selector(".ec-savebar.show", state="hidden")
    ec.wait_for_selector(".chat-head .topic:has-text('Bring flasks')", timeout=10000)
    ec.click("dialog.edit-channel .settings-nav button[data-tab='permissions']")
    ec.wait_for_selector("dialog.edit-channel .ec-perms .perm-row", timeout=10000)
    ec.wait_for_timeout(300)
    ec.screenshot(path=f"{SHOTS}/164-edit-channel-permissions.png")
    ec.click("dialog.edit-channel .settings-nav button[data-tab='delete']")
    ec.wait_for_selector("dialog.edit-channel .ec-danger")
    ec.screenshot(path=f"{SHOTS}/165-edit-channel-delete.png")
    ec.keyboard.press("Escape")
    ec.wait_for_selector("dialog.settings", state="detached")
    # The ⚙ on hover and the right-click menu both open it.
    ec.hover(f"#text-channels .channel[data-id='{spoil['id']}']")
    ec.click(f"#text-channels .channel[data-id='{spoil['id']}'] .ch-gear")
    ec.wait_for_selector("dialog.edit-channel .ec-mode-spoiler input:checked")
    assert ec.evaluate("document.querySelector('.ec-slow-value').textContent") == "30 seconds"
    ec.keyboard.press("Escape")
    ec.wait_for_selector("dialog.settings", state="detached")
    ec.click(f"#text-channels .channel[data-id='{dark['id']}']", button="right")
    ec.wait_for_selector(".menu button:has-text('Edit channel')")
    ec.screenshot(path=f"{SHOTS}/166-channel-menu.png")
    ec.keyboard.press("Escape")
    # Age-restricted: the gate, then the messages once you say yes.
    ec.click(f"#text-channels .channel[data-id='{dark['id']}']")
    ec.wait_for_selector(".age-gate .ag-continue")
    assert ec.evaluate(f"!!document.querySelector(\"#text-channels .channel[data-id='{dark['id']}'] .nsfw-badge\")")
    assert ec.evaluate("getComputedStyle(document.querySelector('#composer')).display") == "none"
    ec.screenshot(path=f"{SHOTS}/167-nsfw-gate.png")
    ec.click(".age-gate .ag-continue")
    ec.wait_for_selector("#messages .msg-text:has-text('Late night banter')", timeout=10000)
    ec.screenshot(path=f"{SHOTS}/168-nsfw-accepted.png")
    # Spoiler channel: blurred until clicked.
    ec.click(f"#text-channels .channel[data-id='{spoil['id']}']")
    ec.wait_for_selector("#messages article.msg.spoilered")
    ec.wait_for_selector(".spoiler-bar")
    ec.screenshot(path=f"{SHOTS}/169-spoiler-channel.png")
    before = ec.evaluate("document.querySelectorAll('#messages article.msg.spoilered').length")
    ec.click("#messages article.msg.spoilered .spoiler-cover >> nth=0")
    ec.wait_for_function(f"document.querySelectorAll('#messages article.msg.spoilered').length === {before - 1}")
    ec.click(".spoiler-bar button:has-text('Show all')")
    ec.wait_for_function("document.querySelectorAll('#messages article.msg.spoilered').length === 0")
    ec.screenshot(path=f"{SHOTS}/170-spoiler-shown.png")
    ec.close()

    # Slowmode countdown for someone who doesn't skip it.
    sd = fresh(dave["token"], "slowmode-desktop", viewport={"width": 1280, "height": 800})
    sd.click(f"#text-channels .channel[data-id='{spoil['id']}']")
    sd.wait_for_selector("#slowmode-note")
    sd.fill("#composer-input", "Who's got the frost resist gear?")
    sd.keyboard.press("Enter")
    sd.wait_for_selector("#slowmode-note.cooling", timeout=10000)
    sd.wait_for_timeout(1200)
    sd.screenshot(path=f"{SHOTS}/171-slowmode-countdown.png")
    sd.fill("#composer-input", "too quick")
    sd.keyboard.press("Enter")
    sd.wait_for_selector(".toast:has-text('Slowmode')", timeout=5000)
    assert sd.evaluate("document.querySelector('#composer-input').value") == "too quick"
    # Dave sees the 18+ gate (Carol's yes doesn't count for him).
    sd.click(f"#text-channels .channel[data-id='{dark['id']}']")
    sd.wait_for_selector(".age-gate")
    sd.close()

    # Phone and Android app.
    api("DELETE", f"/api/channels/{dark['id']}/age-gate", None, TOKEN)
    for label, kw, n in (("channels-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 175),
                         ("android-channels", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 185)):
        pg = fresh(TOKEN, label, **kw)
        pg.wait_for_selector("#channel-name.has-topic")
        pg.screenshot(path=f"{SHOTS}/{n}-{label}-header.png")
        pg.click("#channel-name")
        pg.wait_for_selector(".topic-dialog")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n + 1}-{label}-topic.png")
        pg.click(".topic-dialog button:has-text('Done')")
        pg.evaluate(f"openEditChannel(chById('{chan['id']}'))")
        pg.wait_for_selector("dialog.edit-channel .ec-modes")
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{n + 2}-{label}-edit-channel.png")
        pg.evaluate("document.querySelector('dialog.edit-channel .ec-modes').scrollIntoView({block: 'end'})")
        pg.click("dialog.edit-channel .ec-mode-spoiler")
        pg.wait_for_selector(".ec-savebar.show")
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{n + 3}-{label}-edit-visibility.png")
        pg.click(".ec-savebar button:has-text('Reset')")
        pg.click("dialog.edit-channel .settings-nav button[data-tab='permissions']")
        pg.wait_for_selector("dialog.edit-channel .ec-perms .perm-row", timeout=10000)
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/{n + 4}-{label}-edit-permissions.png")
        pg.keyboard.press("Escape")
        pg.wait_for_selector("dialog.settings", state="detached")
        pg.evaluate(f"selectChannel('{dark['id']}')")
        pg.wait_for_selector(".age-gate .ag-continue")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n + 5}-{label}-nsfw-gate.png")
        pg.evaluate(f"selectChannel('{spoil['id']}')")
        pg.wait_for_selector("#messages article.msg.spoilered")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n + 6}-{label}-spoiler.png")
        pg.close()


    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("UI test passed.")
