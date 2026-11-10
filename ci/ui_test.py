"""Clicks through the web client in headless Chrome against a real server.

Fails on any JavaScript error and saves screenshots (uploaded by CI) so the
new screens can be eyeballed without deploying.
"""

import base64
import json
import os
import sys
import time
import urllib.error
import urllib.parse
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
carol = api("POST", "/api/auth/register", {"username": "carolui", "password": "password123", "birth_date": "1990-01-01", "display_name": "Carol"})
dave = api("POST", "/api/auth/register", {"username": f"dave{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Dave"})
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


erin = api("POST", "/api/auth/register", {"username": f"erin{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Erin"})
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
lightwell = api("POST", "/api/users/@me/characters", {"name": "Lightwell", "last_name": "Dawnstar", "ruleset": "PvP", "race": "Human", "class": "Priest", "level": 60,
    "professions": ["Tailoring", "Enchanting"], "armory_url": "https://example.com/lightwell", "is_main": True}, TOKEN)
# The Armory (ci/wow_mock.py) knows Lightwell: portrait, guild, item level.
synced = api("POST", f"/api/characters/{lightwell['id']}/refresh", None, TOKEN)
assert synced["found"] and synced["character"]["guild_name"] == "Forever Flush" and synced["character"]["item_level"] == 66, synced
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
api("POST", f"/api/channels/{lfgc['id']}/wow/char", {"query": "Lightwell"}, dave["token"])

# ---- Batch 5: voice extras (moderation, entrance sounds, soundboard favourites, screen share) ----
def make_wav(seconds, rate=8000):
    n = int(rate * seconds)
    # A quiet two-tone beep so it's audibly a sound if anyone listens.
    import math
    data = bytes(int(128 + 40 * math.sin(2 * math.pi * (440 if i < n // 2 else 660) * i / rate)) for i in range(n))
    return (b"RIFF" + struct.pack("<I", 36 + n) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate, 1, 8)
            + b"data" + struct.pack("<I", n) + data)


frank = api("POST", "/api/auth/register", {"username": f"frank{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Frank"})
api("POST", f"/api/invites/{code}", None, frank["token"])
FRANK = frank["user"]["id"]
comms = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "Raid comms", "kind": 2}, TOKEN)
afk = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "AFK", "kind": 2}, TOKEN)
upload("/api/users/@me/entrance-sound", {}, {"file": ("hello.wav", make_wav(1), "audio/wav")}, dave["token"])
for name, emoji in [("Leeroy", "🐔"), ("Bonk", "🔨"), ("Wipe", "💀"), ("Bloodlust", "🥁"), ("Ding", "✨")]:
    snd = upload(f"/api/guilds/{guild['id']}/sounds", {"name": name, "emoji": emoji}, {"file": (f"{name}.wav", make_wav(0.5), "audio/wav")}, TOKEN)
    if name in ("Leeroy", "Wipe"):
        api("PUT", f"/api/sounds/{snd['id']}/favourite", None, TOKEN)
# A sound in Dave's Family outhouse (Carol's in it too): her soundboard
# shows it in its own section and can play it in Raid comms.
upload(f"/api/guilds/{family['id']}/sounds", {"name": "Dinner bell", "emoji": "🔔"}, {"file": ("bell.wav", make_wav(0.5), "audio/wav")}, dave["token"])
api("PUT", "/api/sounds/builtin:plop/favourite", None, TOKEN)
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

    # The real Content-Security-Policy blocks eval, and Playwright's
    # wait_for_function compiles its expression with new Function(), so these
    # functional tests bypass CSP. ci/csp_test.py checks the app under the
    # real policy.
    def _bypass_csp(make):
        return lambda *a, **kw: make(*a, **{"bypass_csp": True, **kw})
    browser.new_page = _bypass_csp(browser.new_page)
    browser.new_context = _bypass_csp(browser.new_context)

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
    page.wait_for_selector(".rc-entry[data-role='healer'] .su-name.me", timeout=10000)

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
    page.wait_for_timeout(1500)
    # Opening a channel lands on the newest messages, and stays there while it settles.
    # Opening a channel lands where you stopped reading ("New messages" line near the top),
    # or at the newest message when everything's read.
    pos = page.evaluate("""(() => { const b = document.getElementById('messages'); const d = document.getElementById('new-divider');
      return { gap: b.scrollHeight - b.scrollTop - b.clientHeight, divider: d ? d.getBoundingClientRect().top - b.getBoundingClientRect().top : null }; })()""")
    if pos["divider"] is None:
        assert pos["gap"] < 5, f"channel didn't open at the newest messages: {pos}"
    else:
        assert 0 <= pos["divider"] <= 200 or pos["gap"] < 5, f"channel didn't open at the New messages line: {pos}"
    # Scroll up like a person (mouse wheel), which also stops the pin-to-bottom.
    page.hover("#messages")
    for _ in range(30):
        page.mouse.wheel(0, -2000)
        if page.evaluate("document.getElementById('messages').scrollTop") < 300:
            break
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
    page.wait_for_selector(f"{bench_card} .ev-bench .su-name:has-text('Erin')", timeout=10000)
    page.wait_for_timeout(800)  # let the channel finish settling at the bottom first
    assert "Dave" in page.inner_text(f"{bench_card} .rc-entry[data-role='tank']")
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
    # Raid sizes follow the game: TBC is 10/25, Forever 10/20/40.
    def size_labels():
        return page.eval_on_selector_all(".raid-dialog label.field:has-text('Size') select option", "os => os.map(o => o.textContent)")
    page.select_option(".raid-dialog select[aria-label='Game']", "tbc")
    assert size_labels() == ["Custom", "25-player raid", "10-player raid"], size_labels()
    assert [page.input_value(f".raid-dialog label:has-text('{r}') input[type=number]") for r in ("Tanks", "Healers")] == ["3", "7"]
    page.select_option(".raid-dialog select[aria-label='Game']", "forever")
    assert size_labels() == ["Custom", "40-player raid", "20-player raid", "10-player raid"], size_labels()
    page.select_option(".raid-dialog select[aria-label='Game']", "")
    page.select_option(".raid-dialog label.field:has-text('Size') select", label="40-player raid")
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
    page.wait_for_selector(".calendar-dialog .cal-daypanel .rc-entry[data-role='dps'] .su-name.me", timeout=10000)
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
    page.click(f"{past_card} .ev-link.attendance")
    page.wait_for_selector(".attendance-dialog .att-row:has-text('Erin')")
    page.click(".attendance-dialog .att-all")
    page.click(".attendance-dialog .att-row:has-text('Erin') .att-choice.late")
    page.wait_for_selector(".attendance-dialog .att-row:has-text('Dave') .att-choice.present.on")
    page.screenshot(path=f"{SHOTS}/47-take-attendance.png")
    page.click(".attendance-dialog button:has-text('Save attendance')")
    page.wait_for_selector(".attendance-dialog", state="detached")
    page.wait_for_selector(f"{past_card} .ev-link.attendance.done", timeout=10000)
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
    page.select_option(".char-dialog select[aria-label='Ruleset']", "Roleplay PvP")
    page.screenshot(path=f"{SHOTS}/66-add-character.png")
    page.click(".char-dialog button[type=submit]")
    page.wait_for_selector(".chars-settings .char-row:has-text('Stabbystab Shadowstep'):has-text('Roleplay PvP')", timeout=10000)
    # Armory: Lightwell's card has her portrait, item level and guild, and Armory links.
    page.wait_for_selector(".chars-settings .char-row:has-text('Lightwell') .char-avatar img", timeout=10000)
    page.wait_for_selector(".chars-settings .char-row:has-text('Lightwell') .char-armory:has-text('ilvl 66'):has-text('Forever Flush')")
    href = page.get_attribute(".chars-settings .char-row:has-text('Lightwell') .armory-links a >> nth=0", "href")
    assert href == "https://worldofwarcraft.blizzard.com/en-us/forever/us/armory/character/lightwell-dawnstar", href
    page.evaluate("document.querySelector('.chars-settings .char-row').scrollIntoView({block: 'start'})")
    page.wait_for_timeout(300)
    page.screenshot(path=f"{SHOTS}/130-armory-character-cards.png")
    page.click(".chars-settings .char-row:has-text('Lightwell') .char-refresh")
    page.wait_for_selector(".toast:has-text('Lightwell Dawnstar is up to date'), .toast:has-text('moment ago')", timeout=10000)
    # Look up on the Armory fills in the form.
    page.click(".chars-settings .add-char")
    page.wait_for_selector(".char-dialog[open] .armory-box:not([hidden])")
    page.fill(".char-dialog input[aria-label='Name']", "sunny")
    page.fill(".char-dialog input[aria-label='Last name']", "skyward")
    page.click(".char-dialog .armory-lookup")
    page.wait_for_selector(".char-dialog .armory-found:has-text('Cloud Nine')", timeout=10000)
    assert page.input_value(".char-dialog select[aria-label='Class']") == "Mage"
    assert page.input_value(".char-dialog select[aria-label='Race']") == "Skyborne (Alliance)"
    assert page.input_value(".char-dialog select[aria-label='Ruleset']") == "Roleplay"
    assert page.input_value(".char-dialog input[aria-label='Level']") == "52"
    assert page.input_value(".char-dialog input[aria-label='Name']") == "Sunny"
    page.wait_for_selector(".char-dialog .armory-found .char-avatar img")
    page.wait_for_timeout(300)
    page.screenshot(path=f"{SHOTS}/131-armory-lookup.png")
    page.click(".char-dialog button[type=submit]")
    page.wait_for_selector(".chars-settings .char-row:has-text('Sunny Skyward') .char-armory:has-text('Cloud Nine')", timeout=10000)
    # Not on the Armory: said plainly, nothing else changes.
    page.click(".chars-settings .add-char")
    page.wait_for_selector(".char-dialog[open]")
    page.fill(".char-dialog input[aria-label='Name']", "Ghosty")
    page.fill(".char-dialog input[aria-label='Last name']", "Nobody")
    page.click(".char-dialog .armory-lookup")
    page.wait_for_selector(".char-dialog .armory-not_found:has-text('doesn')", timeout=10000)
    page.screenshot(path=f"{SHOTS}/132-armory-not-found.png")
    page.click(".char-dialog button:has-text('Cancel')")
    page.wait_for_selector(".char-dialog", state="detached")
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
    page.wait_for_selector(".profile-card .pc-armory:has-text('Lightwell Dawnstar'):has-text('Forever Flush') .char-avatar img")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/67-profile-characters.png")
    page.keyboard.press("Escape")
    # /char card with the portrait and Armory links.
    page.click(".channel:has-text('lfg')")
    page.wait_for_selector("#messages .wow-char.has-portrait:has-text('Lightwell') .wow-portrait", timeout=10000)
    page.wait_for_selector("#messages .wow-char:has-text('Lightwell') .armory-links a:has-text('AoTC.gg')")
    page.wait_for_timeout(800)  # the channel settles (images, scroll to the end) first
    page.evaluate("[...document.querySelectorAll('#messages .wow-char.has-portrait')].pop().scrollIntoView({block: 'center'})")
    page.wait_for_timeout(400)
    page.evaluate("[...document.querySelectorAll('#messages .wow-char.has-portrait')].pop().scrollIntoView({block: 'center'})")
    page.wait_for_timeout(200)
    page.screenshot(path=f"{SHOTS}/133-char-card-armory.png")
    # Without Battle.net keys: a note on where an admin switches it on, no look-up box.
    nokeys = browser.new_page(viewport={"width": 1280, "height": 800})
    watch(nokeys, "nokeys")
    nokeys.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")

    def keys_off(route):
        res = route.fetch()
        body = res.json()
        body["bnet_enabled"] = False
        body["armory_enabled"] = False
        route.fulfill(response=res, json=body)

    nokeys.route("**/api/wow/info", keys_off)
    nokeys.route("**/api/bnet/status", lambda route: route.fulfill(json={"enabled": False, "linked": False}))
    nokeys.goto(BASE + "/")
    nokeys.wait_for_selector(".event-card", timeout=15000)
    nokeys.evaluate("openUserSettings('look')")
    nokeys.wait_for_selector(".chars-settings .armory-off-note:has-text('deploy/README.md')", timeout=10000)
    assert not nokeys.query_selector(".chars-settings .char-refresh")
    nokeys.evaluate("document.querySelector('.chars-settings .armory-off-note').scrollIntoView({block: 'center'})")
    nokeys.wait_for_timeout(200)
    nokeys.screenshot(path=f"{SHOTS}/134-armory-keys-off.png")
    nokeys.click(".chars-settings .add-char")
    nokeys.wait_for_selector(".char-dialog[open]")
    assert nokeys.is_hidden(".char-dialog .armory-box")
    nokeys.close()

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
    phone.click(".event-card:has-text('Molten Core run') .ev-link.attendance")
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
    phone.wait_for_selector(".chars-settings .char-row:has-text('Lightwell') .char-avatar img", timeout=10000)
    phone.evaluate("document.querySelector('.chars-settings .char-row').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/135-phone-armory-character-cards.png")
    phone.click(".chars-settings .add-char")
    phone.wait_for_selector(".char-dialog[open] input[aria-label='Last name']")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/117b-phone-character-forever.png")
    phone.fill(".char-dialog input[aria-label='Name']", "Breezy")
    phone.fill(".char-dialog input[aria-label='Last name']", "Cloudsong")
    phone.click(".char-dialog .armory-lookup")
    phone.wait_for_selector(".char-dialog .armory-found:has-text('Throne Room')", timeout=10000)
    assert phone.input_value(".char-dialog select[aria-label='Race']") == "Skyborne (Horde)"
    assert phone.input_value(".char-dialog select[aria-label='Ruleset']") == "Roleplay PvP"
    phone.evaluate("document.querySelector('.char-dialog .armory-box').scrollIntoView({block: 'start'})")
    phone.wait_for_timeout(300)
    phone.screenshot(path=f"{SHOTS}/136-phone-armory-lookup.png")
    phone.select_option(".char-dialog select[aria-label='Game']", "tbc")
    phone.wait_for_selector(".char-dialog input[aria-label='Realm']")
    phone.wait_for_timeout(200)
    phone.screenshot(path=f"{SHOTS}/118-phone-character-tbc.png")
    phone.keyboard.press("Escape")
    phone.wait_for_selector(".char-dialog", state="detached")
    phone.keyboard.press("Escape")
    phone.wait_for_selector("dialog.settings", state="detached")
    phone.evaluate(f"openProfile({json.dumps(CAROL)}, document.querySelector('#messages'))")
    phone.wait_for_selector(".profile-card .pc-armory .char-avatar img", timeout=10000)
    phone.wait_for_timeout(300)
    phone.screenshot(path=f"{SHOTS}/137-phone-profile-armory.png")
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
    droid.fill(".char-dialog input[aria-label='Name']", "Mockadin")
    droid.fill(".char-dialog input[aria-label='Last name']", "Lightforge")
    droid.click(".char-dialog .armory-lookup")
    droid.wait_for_selector(".char-dialog .armory-found:has-text('Forever Flush')", timeout=10000)
    droid.evaluate("document.querySelector('.char-dialog .armory-box').scrollIntoView({block: 'start'})")
    droid.wait_for_timeout(300)
    droid.screenshot(path=f"{SHOTS}/138-android-armory-lookup.png")
    droid.select_option(".char-dialog select[aria-label='Game']", "tbc")
    droid.wait_for_selector(".char-dialog input[aria-label='Realm']")
    droid.wait_for_timeout(200)
    droid.screenshot(path=f"{SHOTS}/119b-android-character-tbc.png")
    droid.keyboard.press("Escape")
    droid.wait_for_selector(".char-dialog", state="detached")
    droid.keyboard.press("Escape")
    droid.wait_for_selector("dialog.settings", state="detached")
    droid.evaluate(f"openProfile({json.dumps(CAROL)}, document.querySelector('#messages'))")
    droid.wait_for_selector(".profile-card .pc-armory .char-avatar img", timeout=10000)
    droid.wait_for_timeout(300)
    droid.screenshot(path=f"{SHOTS}/139-android-profile-armory.png")
    droid.keyboard.press("Escape")
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
    davev.wait_for_selector("#me-mute.forced", timeout=10000)
    davev.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.server", timeout=10000)
    carolv.wait_for_selector(f"#voice-channels .voice-user[data-id='{DAVE}'] .vicon.server", timeout=10000)
    davev.wait_for_function("state.voice.serverMuted === true")
    davev.screenshot(path=f"{SHOTS}/86-server-muted.png")
    # Rejoining keeps the mute.
    davev.click("#voice-panel .icon-btn.danger")
    davev.wait_for_selector("#voice-panel", state="hidden")
    join(davev, "Raid comms")
    davev.wait_for_function("state.voice.serverMuted === true", timeout=10000)
    davev.wait_for_selector("#me-mute.forced")
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Server mute')")
    davev.wait_for_function("state.voice.serverMuted === false", timeout=10000)
    davev.wait_for_selector("#me-mute.forced", state="detached")
    # Dave can talk again: his mic gets published.
    davev.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!p && !p.isMuted; })()", timeout=15000)

    # Server deafen, then undeafen: Dave hears Carol again.
    carolv.click(f"#voice-channels .voice-user[data-id='{DAVE}']", button="right")
    carolv.click(".menu button:has-text('Server deafen')")
    davev.wait_for_function("state.voice.serverDeafened === true", timeout=10000)
    davev.wait_for_selector("#me-mute.forced")
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

    # The built-in classics really decode in Chrome, aren't silent or clipped, and fit the 10 s limit.
    decoded = carolv.evaluate("""async () => {
      const ctx = new OfflineAudioContext(1, 1, 48000);
      const out = {};
      for (const k of ['flush', 'strain', 'plop', 'door', 'fly']) {
        const a = await ctx.decodeAudioData(await (await fetch(`/sounds/builtin/${k}.mp3`)).arrayBuffer());
        const d = a.getChannelData(0);
        let peak = 0, sum = 0;
        for (let i = 0; i < d.length; i++) { const v = Math.abs(d[i]); if (v > peak) peak = v; sum += d[i] * d[i]; }
        out[k] = [a.duration, peak, Math.sqrt(sum / d.length)];
      }
      return out;
    }""")
    print("built-in sounds (seconds, peak, rms):", decoded)
    for k, (secs, peak, rms) in decoded.items():
        assert 0.3 < secs <= 10 and 0.3 < peak <= 1.0 and rms > 0.03, (k, secs, peak, rms)
    # Soundboard: Discord-style popover above the voice panel.
    # Erin (on her phone, in Raid comms) listens; count the clips her client is told to play.
    erinv.evaluate("window.__plays = []; playSound = (url, volume) => window.__plays.push(String(url));")
    carolv.click("#voice-panel .vp-btn.sb-open")
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:flush']", timeout=10000)
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='fav'] .sb-tile[data-id='builtin:plop']")
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='fav'] .sb-tile:has-text('Leeroy')")
    carolv.wait_for_selector(f".sb-board .sb-sec[data-sec='g{family['id']}'] .sb-tile:has-text('Dinner bell')")
    carolv.wait_for_selector(f".sb-board .sb-sec[data-sec='g{guild['id']}'] .sb-tile:has-text('Bonk')")
    order = carolv.eval_on_selector_all(".sb-board .sb-sec", "els => els.map((e) => e.dataset.sec)")
    assert order[0] == "fav" and order[1] == "classics" and order[2] == f"g{guild['id']}" and f"g{family['id']}" in order, order
    assert carolv.eval_on_selector_all(".sb-rail .sb-rail-btn", "els => els.length") == len(order)
    box = carolv.eval_on_selector(".popover.sb-pop", "e => { const r = e.getBoundingClientRect(); const p = document.getElementById('voice-panel').getBoundingClientRect(); return [r.width, r.height, r.bottom <= p.top + 1]; }")
    assert box[0] >= 500 and box[1] >= 460 and box[2], box
    carolv.hover(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:door']")
    carolv.wait_for_timeout(200)
    carolv.screenshot(path=f"{SHOTS}/88-soundboard.png")
    # Play a classic: the board cools down for everyone's sake.
    carolv.click(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:flush'] .sb-play")
    carolv.wait_for_selector(".sb-board.cooling")
    carolv.wait_for_timeout(500)
    carolv.screenshot(path=f"{SHOTS}/88b-soundboard-cooldown.png")
    erinv.wait_for_function("window.__plays.includes('/sounds/builtin/flush.mp3')", timeout=10000)
    # Favourite a classic with its star.
    carolv.hover(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:door']")
    carolv.click(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:door'] .sb-star")
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='fav'] .sb-tile[data-id='builtin:door']")
    # Search filters live; arrow keys move between tiles.
    carolv.fill(".sb-search", "do")
    carolv.wait_for_function("[...document.querySelectorAll('.sb-board .sb-tile')].every((t) => /do/i.test(t.textContent))")
    assert carolv.query_selector(".sb-board .sb-tile[data-id='builtin:door']")
    assert not carolv.query_selector(".sb-board .sb-tile:has-text('Leeroy')")
    carolv.wait_for_timeout(200)
    carolv.screenshot(path=f"{SHOTS}/88c-soundboard-search.png")
    carolv.fill(".sb-search", "")
    carolv.focus(".sb-search")
    carolv.keyboard.press("ArrowDown")
    first = carolv.evaluate("document.activeElement.closest('.sb-tile')?.dataset.id")
    carolv.keyboard.press("ArrowRight")
    second = carolv.evaluate("document.activeElement.closest('.sb-tile')?.dataset.id")
    assert first and second and first != second, (first, second)
    carolv.keyboard.press("ArrowDown")
    assert carolv.evaluate("!!document.activeElement.closest('.sb-tile')")
    # Typing anywhere goes to the search box.
    carolv.keyboard.type("bell")
    carolv.wait_for_function("document.querySelector('.sb-search').value === 'bell'")
    carolv.wait_for_selector(".sb-board .sb-tile:has-text('Dinner bell')")
    # A sound from Carol's other outhouse plays in this voice channel (Enter plays).
    carolv.wait_for_timeout(2100)
    carolv.focus(".sb-board .sb-tile:has-text('Dinner bell') .sb-play")
    carolv.keyboard.press("Enter")
    try:
        erinv.wait_for_function("window.__plays.some((u) => u.startsWith('/files/'))", timeout=10000)
    except Exception:
        print("Erin's plays:", erinv.evaluate("JSON.stringify(window.__plays)"))
        raise
    # Volume button.
    carolv.fill(".sb-search", "")
    carolv.click(".sb-board .sb-vol-btn")
    carolv.wait_for_selector(".sb-board .sb-volume input[type=range]")
    carolv.keyboard.press("Escape")
    carolv.wait_for_selector(".sb-board", state="detached")
    # Reopen: what she played is under Recently used.
    carolv.click("#voice-panel .vp-btn.sb-open")
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='recent'] .sb-tile[data-id='builtin:flush']", timeout=10000)
    carolv.wait_for_selector(".sb-board .sb-sec[data-sec='recent'] .sb-tile:has-text('Dinner bell')")
    # Collapse a section, then jump with the rail.
    carolv.click(".sb-board .sb-sec[data-sec='fav'] .sb-sec-head")
    carolv.wait_for_selector(".sb-board .sb-sec.closed[data-sec='fav']")
    carolv.click(".sb-board .sb-sec[data-sec='fav'] .sb-sec-head")
    carolv.click(f".sb-rail .sb-rail-btn[title='{family['name']}']")
    carolv.wait_for_timeout(600)
    carolv.screenshot(path=f"{SHOTS}/88d-soundboard-other-outhouse.png")
    # Manage sounds: hide a classic, then show it again.
    carolv.click(".sb-board .sb-manage-link")
    carolv.wait_for_selector("dialog.sb-manage .sbm-row.classic[data-id='builtin:strain']", timeout=10000)
    carolv.click("dialog.sb-manage .sbm-row[data-id='builtin:strain'] .sbm-toggle")
    carolv.wait_for_selector("dialog.sb-manage .sbm-row.is-hidden[data-id='builtin:strain']")
    carolv.wait_for_timeout(200)
    carolv.screenshot(path=f"{SHOTS}/88e-manage-sounds.png")
    hidden = api("GET", f"/api/guilds/{guild['id']}/sounds", None, dave["token"])
    assert all(s["id"] != "builtin:strain" for s in hidden), "hidden classic still listed for Dave"
    carolv.click("dialog.sb-manage .sbm-row[data-id='builtin:strain'] .sbm-toggle")
    carolv.wait_for_selector("dialog.sb-manage .sbm-row.classic:not(.is-hidden)[data-id='builtin:strain']")
    carolv.keyboard.press("Escape")
    carolv.wait_for_selector("dialog.sb-manage", state="detached")

    # The same popover as a bottom sheet on a phone and in the Android app.
    for pg, label in ((erinv, "phone"), (frankv, "android")):
        if not pg.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            pg.click("#nav-toggle")
            pg.wait_for_timeout(400)
        pg.click("#voice-panel .vp-btn.sb-open")
        pg.wait_for_selector(".sb-board .sb-sec[data-sec='classics'] .sb-tile[data-id='builtin:fly']", timeout=10000)
        sheet = pg.eval_on_selector(".popover.sb-pop", "e => { const r = e.getBoundingClientRect(); return [r.left, r.width, Math.round(innerHeight - r.bottom)]; }")
        assert sheet[0] == 0 and sheet[1] >= 380 and sheet[2] <= 1, sheet
        pg.wait_for_timeout(300)
        pg.screenshot(path=f"{SHOTS}/88{'f' if label == 'phone' else 'g'}-{label}-soundboard.png")
        pg.tap(".sb-board .sb-search")
        pg.keyboard.type("plo")
        pg.wait_for_selector(".sb-board .sb-tile[data-id='builtin:plop']")
        pg.wait_for_timeout(200)
        assert pg.query_selector(".sb-board"), "soundboard closed while typing"
        pg.keyboard.press("Escape")
        pg.wait_for_selector(".sb-board", state="detached")

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
    davev.click(".entrance-box .entrance-classics button[data-key='door']")
    davev.wait_for_selector(".entrance-box .entrance-classics button.on[data-key='door']", timeout=10000)
    davev.wait_for_selector(".entrance-box button:has-text('Door creak')")
    davev.wait_for_timeout(200)
    davev.screenshot(path=f"{SHOTS}/89b-entrance-classic.png")
    assert api("GET", "/api/users/@me/entrance-sound", None, dave["token"])["builtin"] == "door"
    davev.keyboard.press("Escape")
    frankv.evaluate("openUserSettings('voice')")
    frankv.wait_for_selector(".entrance-box", timeout=10000)
    frankv.evaluate("document.querySelector('.entrance-box').scrollIntoView()")
    frankv.wait_for_timeout(300)
    frankv.screenshot(path=f"{SHOTS}/90-android-entrance-settings.png")


    # ---- Mute / deafen live in the user bar (in voice and out of it) ----
    frankv.keyboard.press("Escape")
    frankv.wait_for_selector("dialog.settings", state="detached")
    for pg in (carolv, erinv, frankv):
        assert pg.query_selector("#voice-panel #me-mute") is None, "mute still in the voice panel"
        assert pg.query_selector(".user-panel #me-mute") and pg.query_selector(".user-panel #me-deafen"), "mute/deafen missing from the user bar"
    assert carolv.eval_on_selector("#voice-panel .vp-buttons", "e => e.children.length") == 3  # share, soundboard, settings
    assert frankv.eval_on_selector("#voice-panel .vp-buttons", "e => e.children.length") == 2  # no share on Android
    # In voice: mute → the mic track is muted; deafen implies mute; undeafen restores the old mute.
    carolv.click("#me-mute")
    carolv.wait_for_selector("#me-mute.off[title='Unmute'][aria-pressed='true']")
    carolv.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !p || p.isMuted; })()", timeout=10000)
    carolv.click("#me-deafen")
    carolv.wait_for_selector("#me-deafen.off[title='Undeafen']")
    carolv.click("#me-deafen")
    carolv.wait_for_selector("#me-deafen:not(.off)[title='Deafen']")
    carolv.wait_for_selector("#me-mute.off[title='Unmute']")
    carolv.click("#me-deafen")
    carolv.click("#me-mute")  # unmuting while deafened undeafens too
    carolv.wait_for_selector("#me-mute:not(.off)[title='Mute']")
    carolv.wait_for_selector("#me-deafen:not(.off)[title='Deafen']")
    carolv.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!p && !p.isMuted; })()", timeout=15000)
    # Keyboard: focus and Space toggles it.
    carolv.focus("#me-mute")
    carolv.keyboard.press("Space")
    carolv.wait_for_selector("#me-mute.off")
    carolv.wait_for_timeout(200)
    carolv.screenshot(path=f"{SHOTS}/91a-desktop-userbar-in-voice.png")
    carolv.keyboard.press("Space")
    carolv.wait_for_selector("#me-mute:not(.off)")
    for pg, label in ((erinv, "phone"), (frankv, "android")):
        if not pg.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            pg.click("#nav-toggle")
            pg.wait_for_timeout(400)
        pg.tap("#me-deafen")
        pg.wait_for_selector("#me-deafen.off")
        box = pg.eval_on_selector(".user-panel", "e => { const r = e.getBoundingClientRect(); const b = [...e.querySelectorAll('button')].filter((x) => x.offsetParent).map((x) => x.getBoundingClientRect()); return [r.right, Math.max(...b.map((x) => x.right)), Math.min(...b.map((x) => x.width))]; }")
        assert box[1] <= box[0] + 0.5 and box[2] >= 30, f"{label} user bar overflows: {box}"
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/91{'b' if label == 'phone' else 'c'}-{label}-userbar-in-voice.png")
        pg.tap("#me-deafen")
        pg.wait_for_selector("#me-deafen:not(.off)")
    # PTT mode: the mic button still blocks push to talk.
    carolv.evaluate("prefs.set('voice_mode', 'ptt')")
    carolv.click("#me-mute")
    carolv.evaluate("setPtt(true)")
    carolv.wait_for_timeout(500)
    assert carolv.evaluate("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !p || p.isMuted; })()"), "PTT opened a muted mic"
    carolv.evaluate("setPtt(false)")
    carolv.click("#me-mute")
    carolv.evaluate("setPtt(true)")
    carolv.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!p && !p.isMuted; })()", timeout=15000)
    carolv.evaluate("setPtt(false)")
    carolv.evaluate("prefs.set('voice_mode', 'vad'); syncMic(); renderVoicePanel()")

    # ---- Soundboard volume is separate from Output volume ----
    ERIN = erin["user"]["id"]
    carolv.wait_for_function(f"!!state.voice.room.remoteParticipants.get({json.dumps(ERIN)})", timeout=10000)
    sbv = carolv.evaluate(f"""() => {{
      const erin = state.voice.room.remoteParticipants.get({json.dumps(ERIN)});
      const out = [];
      setSoundboardLevel(1); prefs.set('output_volume', 1); applyAllVolumes();
      out.push([soundboardGain(0.8), erin.getVolume()]);
      prefs.set('output_volume', 1.6); applyAllVolumes();
      out.push([soundboardGain(0.8), erin.getVolume()]);
      setSoundboardLevel(0.25); applyAllVolumes();
      out.push([soundboardGain(0.8), erin.getVolume()]);
      playSound('/sounds/builtin/plop.mp3', 0.8, true);
      out.push([state.voice.lastSoundGain, outputVolume()]);
      prefs.set('output_volume', 1); setSoundboardLevel(1); applyAllVolumes();
      return out;
    }}""")
    assert abs(sbv[0][0] - 0.32) < 1e-9 and abs(sbv[1][0] - 0.32) < 1e-9, f"output volume changed the soundboard: {sbv}"
    if isinstance(sbv[0][1], (int, float)):  # LiveKit reports the volume once Erin's audio is subscribed
        assert abs(sbv[1][1] - 1.6 * sbv[0][1]) < 1e-6, f"output volume didn't change voices: {sbv}"
        assert abs(sbv[2][1] - sbv[1][1]) < 1e-9, f"soundboard volume changed voices: {sbv}"
    assert abs(sbv[2][0] - 0.08) < 1e-9, f"soundboard volume didn't apply: {sbv}"
    assert abs(sbv[3][0] - 0.08) < 1e-9 and sbv[3][1] == 1.6, sbv
    # The settings slider and the popover slider are one setting.
    carolv.evaluate("openUserSettings('voice')")
    carolv.wait_for_selector("dialog.settings input.sb-level")
    carolv.eval_on_selector("dialog.settings input.sb-level", "e => { e.value = 150; e.dispatchEvent(new Event('input')); }")
    assert carolv.evaluate("soundboardLevel()") == 1.5
    assert carolv.evaluate("prefs.get('output_volume', 1)") == 1, "soundboard slider moved output volume"
    carolv.evaluate("document.querySelector('dialog.settings .sb-level-wrap').scrollIntoView({block: 'center'})")
    carolv.wait_for_timeout(300)
    carolv.screenshot(path=f"{SHOTS}/92a-voice-audio-soundboard-volume.png")
    carolv.keyboard.press("Escape")
    carolv.wait_for_selector("dialog.settings", state="detached")
    carolv.click("#voice-panel .vp-btn.sb-open")
    carolv.wait_for_selector(".sb-board")
    if not carolv.query_selector(".sb-board .sb-volume"):
        carolv.click(".sb-board .sb-vol-btn")
    carolv.wait_for_selector(".sb-volume input[type=range][max='200']")
    assert carolv.eval_on_selector(".sb-volume input[type=range]", "e => e.value") == "150"
    carolv.eval_on_selector(".sb-volume input[type=range]", "e => { e.value = 60; e.dispatchEvent(new Event('input')); }")
    assert carolv.evaluate("soundboardLevel()") == 0.6
    carolv.keyboard.press("Escape")
    carolv.evaluate("setSoundboardLevel(1)")
    frankv.evaluate("openUserSettings('voice')")
    frankv.wait_for_selector("dialog.settings .sb-level-wrap")
    frankv.evaluate("document.querySelector('dialog.settings .sb-level-wrap').scrollIntoView({block: 'center'})")
    frankv.wait_for_timeout(300)
    frankv.screenshot(path=f"{SHOTS}/92b-android-voice-audio-soundboard-volume.png")
    frankv.keyboard.press("Escape")
    for pg in (carolv, davev, erinv, frankv):
        pg.evaluate("leaveVoice(true)")
    carolv.wait_for_timeout(500)

    # Out of voice: the buttons stay, and what you pick is kept for next time.
    for pg in (carolv, erinv, frankv):
        pg.wait_for_selector("#voice-panel", state="hidden", timeout=10000)
        pg.wait_for_selector(".user-panel #me-mute:not(.forced)")
    carolv.click("#me-mute")
    carolv.wait_for_selector("#me-mute.off[title='Unmute']")
    assert carolv.evaluate("localStorage.getItem('outhouse_self_muted')") == "true"
    carolv.screenshot(path=f"{SHOTS}/91d-desktop-userbar-out-of-voice.png")
    for pg, label in ((erinv, "phone"), (frankv, "android")):
        if not pg.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            pg.click("#nav-toggle")
            pg.wait_for_timeout(400)
        pg.tap("#me-mute")
        pg.wait_for_selector("#me-mute.off")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/91{'e' if label == 'phone' else 'f'}-{label}-userbar-out-of-voice.png")
    # Reload: still muted, and joining voice keeps the mic off.
    carolv.reload()
    carolv.wait_for_selector("#voice-channels .channel.voice", timeout=15000)
    carolv.wait_for_selector("#me-mute.off")
    join(carolv, "Raid comms")
    assert carolv.evaluate("state.voice.muted") is True
    carolv.wait_for_timeout(800)
    assert carolv.evaluate("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !p || p.isMuted; })()"), "mic live after joining muted"
    carolv.wait_for_function(f"state.voiceStates.get({json.dumps(CAROL)})?.muted === true", timeout=10000)
    carolv.click("#me-mute")
    carolv.wait_for_function("(() => { const p = state.voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.Microphone); return !!p && !p.isMuted; })()", timeout=15000)
    carolv.evaluate("leaveVoice(true)")
    carolv.wait_for_timeout(300)
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
    b6.hover(".menu button:has-text('Mute channel')")
    b6.wait_for_selector(".menu button:has-text('For 8 hours')")
    b6.screenshot(path=f"{SHOTS}/96b-channel-mute-menu.png")
    b6.click(".menu button:has-text('Until I turn it back on')")
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
    gus = api("POST", "/api/auth/register", {"username": f"gus{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Gus"})
    hank = api("POST", "/api/auth/register", {"username": f"hank{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Hank"})
    ivy = api("POST", "/api/auth/register", {"username": f"ivy{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Ivy"})
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
    assert rs == {"manager_role_ids": [raid_leader["id"]], "default_channel_id": chan["id"],
                  "card_cleanup_series": "next", "card_cleanup_single": "off"}, rs
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
    rm.wait_for_selector(f"{naxx_card} .rc-entry[data-role='healer'] .su-name.me:has-text('Lightwell Dawnstar')", timeout=10000)
    rm.wait_for_selector(f"{naxx_card} .rc-entry[data-role='tank'] .su-name:has-text('Shieldwall Ironfoe')")
    # The roster shows the character; who's playing it is in the tooltip.
    assert "Dave" in rm.get_attribute(f"{naxx_card} .rc-entry[data-role='tank'] .su-name:has-text('Shieldwall Ironfoe')", "title")
    rm.wait_for_selector(f"{naxx_card} .rc-entry[data-role='dps'] .su-name:has-text('Gus')")
    scroll_to_card(rm, "Naxx invite test")
    rm.screenshot(path=f"{SHOTS}/132-roster-characters.png")
    # Export / invite list.
    rm.click(f"{naxx_card} .ev-link.export")
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
    dv.wait_for_selector(f"{naxx_card} .ev-link.export")
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
    dv.click(f"{naxx_card} .rc-entry[data-role='dps'] .su-name:has-text('Gus')")
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
    assert not iv.query_selector(f"{naxx_card} .ev-link.export"), "Ivy got the invite list"
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
        pg.wait_for_selector(f"{naxx_card} .su-name:has-text('Lightwell')")
        pg.screenshot(path=f"{SHOTS}/{n + 1}-{label}-roster.png")
        pg.click(f"{naxx_card} .ev-link.export")
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
    api("PATCH", f"/api/channels/{spoil['id']}", {"content_mode": "spoiler",
                                                   "topic": "New raid talk. Everything is blurred until you click it."}, TOKEN)
    for text in ("Kel'Thuzad drops the Frostmourne-looking sword now??", "Phase 3 adds Guardians of Icecrown, save your cooldowns",
                 "Sapphiron has a new frost breath pattern, hide behind the ice blocks"):
        api("POST", f"/api/channels/{spoil['id']}/messages", {"content": text}, dave["token"])
    api("PATCH", f"/api/channels/{spoil['id']}", {"slowmode_secs": 30}, TOKEN)
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
    sd.wait_for_timeout(2500)
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


    # ---- Manage nicknames: change someone else's from the menu or their card ----
    nm = fresh(TOKEN, "nick-desktop", viewport={"width": 1280, "height": 800})
    nm.wait_for_selector(f"#member-list .member[data-id='{DAVE}']", timeout=10000)
    nm.click(f"#member-list .member[data-id='{DAVE}']", button="right")
    nm.wait_for_selector(".menu button:has-text('Change nickname')")
    nm.wait_for_timeout(150)
    nm.screenshot(path=f"{SHOTS}/195-member-menu-change-nickname.png")
    nm.click(".menu button:has-text('Change nickname')")
    nm.wait_for_selector("dialog.nick-dialog[open] input[placeholder='Dave']")
    assert nm.eval_on_selector("dialog.nick-dialog input", "e => e.value") == ""
    assert nm.eval_on_selector("dialog.nick-dialog .nick-reset", "e => e.disabled")
    nm.fill("dialog.nick-dialog input", "Tank Dave")
    nm.wait_for_timeout(100)
    nm.screenshot(path=f"{SHOTS}/196-change-nickname-dialog.png")
    nm.click("dialog.nick-dialog .nick-save")
    nm.wait_for_selector("dialog.nick-dialog", state="detached")
    nm.wait_for_selector(f"#member-list .member[data-id='{DAVE}']:has-text('Tank Dave')", timeout=10000)
    # Dave's own client updates live.
    dv = fresh(dave["token"], "nick-dave", viewport={"width": 1280, "height": 800})
    dv.wait_for_selector(f"#member-list .member[data-id='{DAVE}']:has-text('Tank Dave')", timeout=10000)
    # Dave (no Manage nicknames) can't rename Carol; his own is still there for him.
    dv.click(f"#member-list .member[data-id='{CAROL}']", button="right")
    dv.wait_for_selector(".menu button:has-text('View profile')")
    assert dv.query_selector(".menu button:has-text('Change nickname')") is None, "Change nickname shown without permission"
    dv.keyboard.press("Escape")
    dv.click(f"#member-list .member[data-id='{DAVE}']", button="right")
    dv.wait_for_selector(".menu button:has-text('Change nickname')")
    dv.keyboard.press("Escape")
    # The profile card: open it, Change nickname, Reset.
    nm.click(f"#member-list .member[data-id='{DAVE}'] .avatar-wrap")
    nm.wait_for_selector(".profile-card .pc-nick")
    nm.wait_for_timeout(300)
    nm.screenshot(path=f"{SHOTS}/197-profile-card-change-nickname.png")
    nm.click(".profile-card .pc-nick")
    nm.wait_for_selector("dialog.nick-dialog[open]")
    assert nm.eval_on_selector("dialog.nick-dialog input", "e => e.value") == "Tank Dave"
    nm.click("dialog.nick-dialog .nick-reset")
    nm.wait_for_selector("dialog.nick-dialog", state="detached")
    nm.wait_for_selector(f"#member-list .member[data-id='{DAVE}']:not(:has-text('Tank Dave'))", timeout=10000)
    dv.wait_for_selector(f"#member-list .member[data-id='{DAVE}']:not(:has-text('Tank Dave'))", timeout=10000)
    # A moderator with Manage nicknames: fine on Dave, never on the owner.
    nickrole = api("POST", f"/api/guilds/{G}/roles", {"name": "Nick mod", "permissions": "1024"}, TOKEN)
    api("PATCH", f"/api/guilds/{G}/roles/{nickrole['id']}", {"position": 500}, TOKEN)
    api("PUT", f"/api/guilds/{G}/members/{erin['user']['id']}/roles/{nickrole['id']}", None, TOKEN)
    ev = fresh(erin["token"], "nick-erin", viewport={"width": 1280, "height": 800})
    ev.wait_for_selector(f"#member-list .member[data-id='{DAVE}']", timeout=10000)
    ev.click(f"#member-list .member[data-id='{DAVE}']", button="right")
    ev.wait_for_selector(".menu button:has-text('Change nickname')")
    ev.keyboard.press("Escape")
    ev.click(f"#member-list .member[data-id='{CAROL}']", button="right")
    ev.wait_for_selector(".menu button:has-text('View profile')")
    assert ev.query_selector(".menu button:has-text('Change nickname')") is None, "moderator offered to rename the owner"
    ev.keyboard.press("Escape")
    ev.close()
    dv.close()
    # Role editor lists it right after Change nickname.
    nm.evaluate("openSettings('roles')")
    nm.wait_for_selector("dialog.settings :text('Manage nicknames')", timeout=10000)
    order = nm.evaluate("state.permList.map((p) => p.key)")
    assert order.index("manage_nicknames") == order.index("change_nickname") + 1, order
    nm.keyboard.press("Escape")
    nm.close()
    # Phone width: long-press menu and the dialog.
    for label, kw, n in (("nick-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 1960),
                         ("android-nick", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 1980)):
        pg = fresh(TOKEN, label, **kw)
        pg.click("#members-toggle")
        pg.wait_for_timeout(400)
        row = f"#member-list .member[data-id='{DAVE}']"
        pg.wait_for_selector(row)
        pg.eval_on_selector(row, "e => { const r = e.getBoundingClientRect(); e.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 40, clientY: r.top + r.height / 2 })); }")
        pg.wait_for_selector(".menu button:has-text('Change nickname')")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n}-{label}-member-menu.png")
        pg.click(".menu button:has-text('Change nickname')")
        pg.wait_for_selector("dialog.nick-dialog[open]")
        pg.fill("dialog.nick-dialog input", "Dave from the dunny")
        pg.wait_for_timeout(200)
        pg.screenshot(path=f"{SHOTS}/{n + 1}-{label}-nickname-dialog.png")
        pg.keyboard.press("Escape")
        pg.wait_for_selector("dialog.nick-dialog", state="detached")
        pg.close()


    # ---- Old raid cards removed from chat after the raid ----
    # A weekly raid that's on now (every card goes up straight away). Then it
    # finishes, after the next raid's card went up, so its card leaves chat.
    tidy_ch = api("POST", f"/api/guilds/{G}/channels", {"name": "weekly-raids"}, TOKEN)
    tstart = time.time() - 10 * 60
    tidy = api("POST", f"/api/channels/{tidy_ch['id']}/events", {
        "title": "AQ20 weekly", "description": "Ruins of Ahn'Qiraj, bring nature resist.", "starts_at": iso(tstart), "duration_minutes": 15,
        "post_rule": {"mode": "now"}, "repeat": {"weekdays": [time.gmtime(tstart).tm_wday], "every_weeks": 1, "count": 3, "time_zone": "UTC"},
    }, TOKEN)
    assert tidy["event"]["message_id"] and tidy["event"]["card_cleanup"] == "next", tidy
    TID = tidy["event"]["id"]
    import subprocess
    subprocess.run(["psql", os.environ["DATABASE_URL"], "-qc", f"UPDATE events SET starts_at = starts_at - interval '20 minutes' WHERE id = {TID}"], check=True)
    tstart -= 20 * 60
    later_ids = [e["id"] for e in api("GET", f"/api/guilds/{G}/events", None, TOKEN) if e.get("series_id") == tidy["event"]["series_id"]]
    assert len(later_ids) == 2, later_ids
    api("PATCH", f"/api/events/{later_ids[0]}", {"description": "Ruins of Ahn'Qiraj, bring nature resist. Week 2."}, TOKEN)  # runs the clean-up now
    gone = api("GET", f"/api/events/{TID}", None, TOKEN)
    assert gone["card_removed"] and gone["message_id"] is None, gone
    api("PUT", f"/api/events/{TID}/signup", {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection", "user_id": DAVE}, TOKEN)
    api("PUT", f"/api/events/{TID}/signup", {"status": "yes", "role": "dps", "class": "Mage", "spec": "Frost", "user_id": ERIN}, TOKEN)
    cleanup_sel = ".raid-dialog select[aria-label='Remove the chat card after the raid']"
    cleanup_opts = "s => [...s.options].map((o) => o.value)"
    td = fresh(TOKEN, "tidy-desktop", viewport={"width": 1280, "height": 800})
    td.click(".channel:has-text('weekly-raids')")
    td.wait_for_selector("#messages .event-card:has-text('AQ20 weekly')", timeout=10000)
    td.wait_for_timeout(300)
    n_cards = td.evaluate("document.querySelectorAll('#messages .event-card').length")
    assert n_cards == 2, f"expected only the next two raids' cards in chat, got {n_cards}"
    # Plan a raid: Off for one-off raids, "when the next raid's sign-ups go up" once it repeats.
    td.click("#raid-btn")
    td.wait_for_selector(".raid-list button.primary")
    td.click(".raid-list button.primary")
    td.wait_for_selector(".raid-dialog[open]")
    assert td.input_value(cleanup_sel) == "off"
    assert td.eval_on_selector(cleanup_sel, cleanup_opts) == ["off", "after"]
    td.select_option(".raid-dialog select[aria-label='Repeat']", "1")
    assert td.input_value(cleanup_sel) == "next"
    assert td.eval_on_selector(cleanup_sel, cleanup_opts) == ["off", "after", "next"]
    td.fill(".raid-dialog input[maxlength='100']", "Molten Core weekly")
    td.eval_on_selector(".raid-dialog .cleanup-field", "e => e.scrollIntoView({block: 'center'})")
    td.wait_for_timeout(200)
    td.screenshot(path=f"{SHOTS}/2000-plan-raid-card-cleanup.png")
    td.keyboard.press("Escape")
    td.wait_for_selector(".raid-dialog", state="detached")
    # Edit this and future: the series' choice; editing the raid whose card went says so.
    td.evaluate(f"api('GET', '/api/events/{later_ids[0]}').then((ev) => openRaidDialog(ev, {{ scope: 'future' }}))")
    td.wait_for_selector(".raid-dialog[open] h2:has-text('Edit this and future raids')")
    assert td.input_value(cleanup_sel) == "next"
    td.keyboard.press("Escape")
    td.wait_for_selector(".raid-dialog", state="detached")
    td.evaluate(f"api('GET', '/api/events/{TID}').then((ev) => openRaidDialog(ev))")
    td.wait_for_selector(".raid-dialog[open] .cleanup-gone")
    td.keyboard.press("Escape")
    td.wait_for_selector(".raid-dialog", state="detached")
    # The calendar shows the whole raid (roster, invite list, Take attendance) without a chat card.
    td.evaluate(f"openCalendar(new Date({json.dumps(iso(tstart))}))")
    td.wait_for_selector(".calendar-dialog .cal-tz")
    td.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    tcard = ".calendar-dialog .cal-daypanel .event-card:has-text('AQ20 weekly')"
    td.wait_for_selector(f"{tcard} .ev-link.attendance:has-text('Take attendance')", timeout=10000)
    td.wait_for_selector(f"{tcard} .ev-link.export")
    td.wait_for_selector(f"{tcard} .rc-entry[data-role='tank'] .su-name")
    td.eval_on_selector(tcard, "e => e.scrollIntoView({block: 'center'})")
    td.wait_for_timeout(200)
    td.screenshot(path=f"{SHOTS}/2001-calendar-raid-without-card.png")
    td.click(f"{tcard} .ev-link.attendance")
    td.wait_for_selector(".attendance-dialog .att-row", timeout=10000)
    td.click(".attendance-dialog .att-all")
    td.wait_for_timeout(200)
    td.screenshot(path=f"{SHOTS}/2002-attendance-without-card.png")
    td.click(".attendance-dialog button.primary:has-text('Save attendance')")
    td.wait_for_selector(".attendance-dialog", state="detached", timeout=10000)
    for _ in range(30):
        if api("GET", f"/api/events/{TID}", None, TOKEN)["attendance_taken"]:
            break
        td.wait_for_timeout(200)
    assert api("GET", f"/api/events/{TID}", None, TOKEN)["attendance_taken"]
    td.click(".calendar-dialog button[title='Close']")
    td.wait_for_selector(".calendar-dialog", state="detached")
    # Opening it from anywhere else (raid list rows use the same path) shows it in a window.
    td.evaluate(f"api('GET', '/api/events/{TID}').then(openRaid)")
    td.wait_for_selector(".raid-card-dialog .event-card:has-text('AQ20 weekly')", timeout=10000)
    td.wait_for_selector(".raid-card-dialog .raid-card-head:has-text('chat card was removed')")
    td.wait_for_selector(".raid-card-dialog .ev-link.attendance:has-text('Attendance ✓')")
    td.screenshot(path=f"{SHOTS}/2003-raid-window-without-card.png")
    td.keyboard.press("Escape")
    td.close()
    # Phone width: the dialog option and the calendar agenda.
    tp = fresh(TOKEN, "tidy-phone", viewport={"width": 390, "height": 844}, is_mobile=True, has_touch=True)
    tp.evaluate("openRaidDialog()")
    tp.wait_for_selector(".raid-dialog[open]")
    tp.select_option(".raid-dialog select[aria-label='Repeat']", "1")
    assert tp.input_value(cleanup_sel) == "next"
    tp.eval_on_selector(".raid-dialog .cleanup-field", "e => e.scrollIntoView({block: 'center'})")
    tp.wait_for_timeout(200)
    tp.screenshot(path=f"{SHOTS}/2004-phone-plan-raid-card-cleanup.png")
    tp.keyboard.press("Escape")
    tp.wait_for_selector(".raid-dialog", state="detached")
    tp.evaluate(f"openCalendar(new Date({json.dumps(iso(tstart))}))")
    tp.wait_for_selector(".calendar-dialog .cal-tz")
    tp.wait_for_function("!document.querySelector('.calendar-dialog .cal-tz').textContent.includes('Loading')")
    tp.click(".calendar-dialog .cal-agenda-row:has-text('AQ20 weekly') >> nth=0")
    tp.wait_for_selector(".calendar-dialog .cal-agenda-item.open .event-card .ev-link.attendance", timeout=10000)
    tp.eval_on_selector(".calendar-dialog .cal-agenda-item.open .event-card", "e => e.scrollIntoView({block: 'start'})")
    tp.wait_for_timeout(200)
    tp.screenshot(path=f"{SHOTS}/2005-phone-calendar-raid-without-card.png")
    tp.close()

    # ---- Changing a repeating raid's pattern, thread mention badges, muted DMs ----
    CH = chan["id"]
    pat_ch = api("POST", f"/api/guilds/{G}/channels", {"name": "pattern-raids"}, TOKEN)
    pday = time.gmtime(time.time() + 86400)
    pser = api("POST", f"/api/channels/{pat_ch['id']}/events", {
        "title": "Naxx weekly", "description": "Naxxramas, spider and plague wings.", "starts_at": time.strftime("%Y-%m-%dT19:00:00Z", pday),
        "post_rule": {"mode": "now"}, "repeat": {"weekdays": [pday.tm_wday], "every_weeks": 1, "count": 8, "time_zone": "UTC"},
    }, TOKEN)
    prange = f"from={iso(time.time() - 3600)}&to={iso(time.time() + 40 * 86400)}"
    pevs = lambda sid: [e for e in api("GET", f"/api/guilds/{G}/events?{prange}", None, TOKEN) if e.get("series_id") == sid]
    pids = [e["id"] for e in pevs(pser["event"]["series_id"])]
    assert len(pids) >= 4, pids
    for who in (DAVE, ERIN):
        api("PUT", f"/api/events/{pids[2]}/signup", {"status": "yes", "role": "dps", "class": "Rogue", "spec": "Combat", "user_id": who}, TOKEN)
    api("PUT", f"/api/events/{pids[1]}/signup", {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection", "user_id": DAVE}, TOKEN)

    def pattern_dialog(pg, label, shot):
        pg.evaluate(f"api('GET', '/api/events/{pids[1]}').then((ev) => openRaidDialog(ev, {{ scope: 'future' }}))")
        pg.wait_for_selector(".raid-dialog[open] h2:has-text('Edit this and future raids')")
        pg.wait_for_selector(".raid-dialog .pattern-preview .pp-dates li >> nth=3", timeout=10000)
        assert pg.input_value(".raid-dialog select[aria-label='Repeat']") == "1"
        assert pg.eval_on_selector_all(".raid-dialog .day-chip.on", "els => els.length") == 1
        pg.select_option(".raid-dialog select[aria-label='Repeat']", "2")
        pg.wait_for_selector(".raid-dialog .pattern-preview .pattern-warn", timeout=10000)
        warn = pg.text_content(".raid-dialog .pattern-warn")
        assert "cancelled" in warn and "2 sign-ups" in warn and "Outhouse Helper" in warn, warn
        assert "Every 2 weeks" in pg.text_content(".raid-dialog .pattern-preview .pp-head")
        pg.eval_on_selector(".raid-dialog .pattern-preview", "e => e.scrollIntoView({block: 'center'})")
        pg.wait_for_timeout(250)
        pg.screenshot(path=f"{SHOTS}/{shot}-{label}-raid-pattern-edit.png")

    pd = fresh(TOKEN, "pattern-desktop", viewport={"width": 1280, "height": 800})
    pattern_dialog(pd, "desktop", 2100)
    for label, kw in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}),
                      ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True})):
        pp = fresh(TOKEN, f"pattern-{label}", **kw)
        pattern_dialog(pp, label, 2101 if label == "phone" else 2102)
        pp.keyboard.press("Escape")
        pp.close()
    # Save it on the desktop: the 3rd raid (Dave and Erin) goes, the 2nd keeps Dave.
    pd.click(".raid-dialog button.primary:has-text('Save')")
    pd.wait_for_selector(".raid-dialog", state="detached", timeout=10000)
    for _ in range(40):
        try:
            api("GET", f"/api/events/{pids[2]}", None, TOKEN)
        except urllib.error.HTTPError as e:
            assert e.code == 404, e
            break
        pd.wait_for_timeout(250)
    else:
        raise AssertionError("the raid that no longer fits wasn't cancelled")
    kept = api("GET", f"/api/events/{pids[1]}", None, TOKEN)
    assert kept["repeat"]["every_weeks"] == 2 and kept["series_id"] != pser["event"]["series_id"] and len(kept["signups"]) == 1, kept
    pd.close()

    # Threads: a plain reply (dot) and @mentions (count) for Carol.
    tmsg = api("POST", f"/api/channels/{CH}/messages", {"content": "Loot council for Naxx: who needs what?"}, dave["token"])
    t_loot = api("POST", f"/api/channels/{CH}/threads", {"name": "Loot council", "message_id": tmsg["id"]}, dave["token"])
    api("POST", f"/api/channels/{CH}/messages", {"content": "I'll take the shoulders if nobody else needs them", "thread_id": t_loot["id"]}, erin["token"])
    t_pull = api("POST", f"/api/channels/{CH}/threads", {"name": "Pull timers"}, dave["token"])
    api("POST", f"/api/channels/{CH}/messages", {"content": f"<@{CAROL}> can you call the pulls?", "thread_id": t_pull["id"]}, dave["token"])
    api("POST", f"/api/channels/{CH}/messages", {"content": "@everyone 10 second pull timer from now on", "thread_id": t_pull["id"]}, dave["token"])
    th = fresh(TOKEN, "threads-desktop", viewport={"width": 1280, "height": 800})
    th.wait_for_selector("#threads-btn .tool-badge", timeout=10000)
    assert th.text_content("#threads-btn .tool-badge") == "2"
    th.wait_for_selector(".thread-link:has-text('Loot council') .thread-dot")
    before = th.evaluate(f"guildMentions({json.dumps(G)})")
    th.click("#threads-btn")
    pull_card = ".threads-panel .thread-card:has-text('Pull timers')"
    loot_card = ".threads-panel .thread-card:has-text('Loot council')"
    th.wait_for_selector(f"{pull_card} .thread-badge", timeout=10000)
    assert th.text_content(f"{pull_card} .thread-badge") == "2"
    th.wait_for_selector(f"{loot_card}.unread .thread-dot")
    th.wait_for_timeout(200)
    th.screenshot(path=f"{SHOTS}/2103-desktop-thread-list-badges.png")
    # Opening it reads it: its count leaves the thread, the channel and the outhouse.
    th.click(pull_card)
    th.wait_for_selector(".thread-panel .tmsg:has-text('pull timer')", timeout=10000)
    th.wait_for_function(f"!state.threadUnread.has({json.dumps(t_pull['id'])})", timeout=10000)
    assert th.evaluate(f"guildMentions({json.dumps(G)})") == before - 2
    for _ in range(20):
        if all(t["thread_id"] != t_pull["id"] for t in api("GET", "/api/users/@me/unread", None, TOKEN)["threads"]):
            break
        th.wait_for_timeout(250)
    else:
        raise AssertionError("opening the thread didn't mark it read on the server")
    # A new @mention arrives live.
    th.click("#threads-btn")
    th.wait_for_selector(loot_card, timeout=10000)
    api("POST", f"/api/channels/{CH}/messages", {"content": f"<@{CAROL}> you're on loot rolls", "thread_id": t_loot["id"]}, dave["token"])
    th.wait_for_selector(f"{loot_card} .thread-badge", timeout=10000)
    assert th.text_content(f"{loot_card} .thread-badge") == "1"
    assert not th.query_selector(f"{pull_card} .thread-badge, {pull_card} .thread-dot")
    th.screenshot(path=f"{SHOTS}/2104-desktop-thread-badge-live.png")
    th.close()
    for label, kw, shot in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2105),
                            ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2106)):
        tpg = fresh(TOKEN, f"threads-{label}", **kw)
        tpg.wait_for_selector("#threads-btn .tool-badge", timeout=10000)
        tpg.click("#threads-btn")
        tpg.wait_for_selector(f"{loot_card} .thread-badge", timeout=10000)
        tpg.wait_for_timeout(200)
        tpg.screenshot(path=f"{SHOTS}/{shot}-{label}-thread-list-badges.png")
        tpg.close()

    # Muted DMs: greyed out, no Home count; mentions in a muted group DM still count.
    ddm = api("POST", "/api/dms", {"user_ids": [CAROL]}, dave["token"])
    api("POST", f"/api/channels/{ddm['id']}/messages", {"content": "did you see the loot list?"}, dave["token"])
    edm = api("POST", "/api/dms", {"user_ids": [CAROL]}, erin["token"])
    api("POST", f"/api/channels/{edm['id']}/messages", {"content": "heals or dps tonight?"}, erin["token"])
    gdm = api("POST", "/api/dms", {"user_ids": [CAROL, ERIN], "name": "Naxx planning"}, dave["token"])
    api("PUT", f"/api/users/@me/notifications/channels/{ddm['id']}", {"muted": True, "minutes": 480}, TOKEN)
    api("PUT", f"/api/users/@me/notifications/channels/{gdm['id']}", {"muted": True}, TOKEN)
    api("POST", f"/api/channels/{ddm['id']}/messages", {"content": "ping me when you're on"}, dave["token"])
    api("POST", f"/api/channels/{gdm['id']}/messages", {"content": "chatter nobody needs to hear"}, dave["token"])
    api("POST", f"/api/channels/{gdm['id']}/messages", {"content": f"<@{CAROL}> you're tanking Patchwerk"}, dave["token"])
    dmp = fresh(TOKEN, "dm-mute-desktop", viewport={"width": 1280, "height": 800})
    dmp.evaluate("openHome()")
    drow = lambda i: f"#dm-list .dm-row[data-id='{i}']"
    dmp.wait_for_selector(f"{drow(ddm['id'])}.muted-dm .mute-icon", timeout=10000)
    dmp.wait_for_selector(f"{drow(gdm['id'])}.muted-dm .mention-badge", timeout=10000)
    assert dmp.text_content(f"{drow(gdm['id'])} .mention-badge") == "1"
    assert not dmp.query_selector(f"{drow(ddm['id'])} .mention-badge") and not dmp.query_selector(f"{drow(ddm['id'])}.unread")
    counts = dmp.evaluate(f"""(() => {{ const u = state.unread.get({json.dumps(ddm['id'])}); const g = state.unread.get({json.dumps(gdm['id'])});
        return {{ raw: u.mentions, counted: ownCount({json.dumps(ddm['id'])}, u), group: ownCount({json.dumps(gdm['id'])}, g) }}; }})()""")
    assert counts["raw"] >= 2 and counts["counted"] == 0 and counts["group"] == 1, counts
    dmp.click(drow(edm["id"]), button="right")
    dmp.wait_for_selector(".menu button:has-text('Mute conversation')")
    dmp.hover(".menu button:has-text('Mute conversation')")
    dmp.wait_for_selector(".menu button:has-text('Until I turn it back on')")
    dmp.wait_for_timeout(200)
    dmp.screenshot(path=f"{SHOTS}/2107-desktop-dm-mute-menu.png")
    dmp.click(".menu button:has-text('For 1 hour')")
    dmp.wait_for_selector(f"{drow(edm['id'])}.muted-dm", timeout=10000)
    mutes = api("GET", "/api/users/@me/notifications", None, TOKEN)["muted"]
    assert any(m["channel_id"] == edm["id"] and m["until"] for m in mutes), mutes
    dmp.wait_for_timeout(300)
    dmp.screenshot(path=f"{SHOTS}/2108-desktop-muted-dms.png")
    dmp.click(drow(edm["id"]), button="right")
    dmp.click(".menu button:has-text('Unmute conversation')")
    dmp.wait_for_selector(f"{drow(edm['id'])}:not(.muted-dm)", timeout=10000)
    # The group DM's ⋯ menu in the header has it too.
    dmp.click(drow(gdm["id"]))
    dmp.wait_for_selector("#dm-tools:not([hidden])", timeout=10000)
    dmp.click("#dm-tools")
    dmp.wait_for_selector(".menu button:has-text('Unmute conversation')")
    dmp.keyboard.press("Escape")
    dmp.close()
    for label, kw, shot in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2109),
                            ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2110)):
        mp = fresh(TOKEN, f"dm-mute-{label}", **kw)
        mp.click("#nav-toggle")
        mp.wait_for_timeout(400)
        mp.click("#home-btn")
        mp.wait_for_selector(f"{drow(ddm['id'])}.muted-dm .mute-icon", timeout=10000)
        if not mp.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            mp.click("#nav-toggle")
            mp.wait_for_timeout(400)
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{shot}-{label}-muted-dm-list.png")
        mp.close()

    # TBC Anniversary and Classic Era characters get "View on Armory" links too.
    api("POST", "/api/users/@me/characters", {"game": "era", "name": "Oldtimer", "realm": "Old Blanchy", "region": "eu", "race": "Dwarf", "class": "Hunter", "level": 60}, TOKEN)
    ar = fresh(TOKEN, "armory-classic", viewport={"width": 1280, "height": 800})
    ar.evaluate("openUserSettings('look')")
    ar.wait_for_selector(".chars-settings .char-row:has-text('Gemcutter') .armory-links a", timeout=10000)
    tbc_href = ar.get_attribute(".chars-settings .char-row:has-text('Gemcutter') .armory-links a >> nth=0", "href")
    assert tbc_href == "https://worldofwarcraft.blizzard.com/en-us/classicann/us/armory/character/spineshatter/gemcutter", tbc_href
    era_href = ar.get_attribute(".chars-settings .char-row:has-text('Oldtimer') .armory-links a >> nth=0", "href")
    assert era_href == "https://worldofwarcraft.blizzard.com/en-us/classic1x/eu/armory/character/old-blanchy/oldtimer", era_href
    ar.evaluate("[...document.querySelectorAll('.chars-settings .char-row')].find((r) => r.textContent.includes('Gemcutter'))?.scrollIntoView({block: 'center'})")
    ar.wait_for_timeout(250)
    ar.screenshot(path=f"{SHOTS}/2113-desktop-classic-armory-links.png")
    ar.click(".chars-settings .char-row:has-text('Gemcutter') button:has-text('Edit')")
    ar.wait_for_selector(".char-dialog select[aria-label='Armory region']", timeout=10000)
    assert ar.input_value(".char-dialog select[aria-label='Armory region']") == "us"
    ar.wait_for_timeout(200)
    ar.screenshot(path=f"{SHOTS}/2114-desktop-tbc-character-region.png")
    ar.close()

    # ---- Bot buttons and menus, ephemeral replies, automod, archived threads ----
    import subprocess
    BM = guild["id"]
    games = api("POST", f"/api/guilds/{BM}/channels", {"name": "bot-games"}, TOKEN)
    roller = api("POST", f"/api/guilds/{BM}/bots", {"name": "Roller"}, TOKEN)
    dice_proc = subprocess.Popen(["node", "web/bots/dice-bot.mjs"], env={**os.environ, "OUTHOUSE_URL": BASE, "OUTHOUSE_BOT_TOKEN": roller["token"]},
                                 stdout=open("/tmp/ui-dice-bot.log", "w"), stderr=subprocess.STDOUT)
    roll_cmd = None
    for _ in range(40):
        roll_cmd = next((c for c in api("GET", f"/api/guilds/{BM}/commands", None, TOKEN) if c["name"] == "roll"), None)
        if roll_cmd:
            break
        time.sleep(0.5)
    assert roll_cmd, open("/tmp/ui-dice-bot.log").read()
    api("POST", f"/api/channels/{games['id']}/messages", {"content": "Who's rolling for the Onyxia bag?"}, dave["token"])
    api("POST", f"/api/channels/{games['id']}/interactions", {"command_id": roll_cmd["id"], "options": {"dice": "d20"}}, TOKEN)
    for _ in range(40):
        msgs = api("GET", f"/api/channels/{games['id']}/messages", None, TOKEN)
        if msgs and msgs[-1].get("components"):
            break
        time.sleep(0.25)
    roll_msg = msgs[-1]
    assert roll_msg["components"][0]["components"][0]["label"] == "Roll again", roll_msg
    # A bot that's online but never answers (for "This interaction failed").
    sleepy = api("POST", f"/api/guilds/{BM}/bots", {"name": "Sleepy"}, TOKEN)
    sleepy_proc = subprocess.Popen(["node", "ci/gateway_probe.mjs", BASE, sleepy["token"], "/tmp/ui-sleepy.jsonl", "600"])
    time.sleep(1.5)
    api("POST", f"/api/channels/{games['id']}/messages", {"content": "I never answer, sorry.", "components": [
        {"type": 1, "components": [{"type": 2, "style": 3, "label": "Wake up", "custom_id": "wake"},
                                    {"type": 2, "style": 4, "label": "Poke", "emoji": {"name": "👉"}, "custom_id": "poke"},
                                    {"type": 2, "style": 5, "label": "Bot docs", "url": "https://example.com/bots"}]}]}, sleepy["token"])

    def bm_page(token, label, channel_id, **kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if label.startswith("android") else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(BM)}); localStorage.setItem('outhouse_channel_{BM}', {json.dumps(channel_id)});")
        pg.goto(BASE + "/")
        pg.wait_for_selector("#messages .msg", timeout=15000)
        return pg

    rollsel = f"#msg-{roll_msg['id']}"
    bm = bm_page(TOKEN, "botsmod-desktop", games["id"], viewport={"width": 1280, "height": 800})
    bm.wait_for_selector(f"{rollsel} .msg-components .mc-btn.mc-primary", timeout=10000)
    assert bm.evaluate(f"document.querySelector('{rollsel} .mc-select').getAttribute('aria-haspopup')") == "listbox"
    bm.screenshot(path=f"{SHOTS}/2501-desktop-bot-buttons.png")
    # Roll again: spinner, then the bot changes the message.
    bm.click(f"{rollsel} .mc-btn:has-text('Roll again')")
    bm.wait_for_selector(f"{rollsel} .mc-btn.loading", state="detached", timeout=5000)
    assert not bm.query_selector(f"{rollsel} .mc-failed")
    # Roll in secret: an ephemeral reply only Carol sees.
    bm.click(f"{rollsel} .mc-btn:has-text('Roll in secret')")
    bm.wait_for_selector("#messages .msg.ephemeral .ephemeral-note:has-text('Only you can see this')", timeout=5000)
    bm.wait_for_timeout(300)
    bm.screenshot(path=f"{SHOTS}/2502-desktop-ephemeral-reply.png")
    eph_id = bm.get_attribute("#messages .msg.ephemeral", "data-id")
    assert all(m["id"] != eph_id for m in api("GET", f"/api/channels/{games['id']}/messages", None, dave["token"])), "ephemeral message was saved"
    # The dice-size menu, with the keyboard.
    bm.click(f"{rollsel} .mc-select")
    bm.wait_for_selector(".mc-pop .mc-listbox[role=listbox] .mc-option", timeout=5000)
    assert bm.evaluate(f"document.querySelector('{rollsel} .mc-select').getAttribute('aria-expanded')") == "true"
    bm.keyboard.press("ArrowDown")
    bm.keyboard.press("ArrowDown")
    bm.screenshot(path=f"{SHOTS}/2503-desktop-bot-select-open.png")
    n_before = bm.evaluate("state.messages.length")
    bm.keyboard.press("Enter")
    bm.wait_for_function(f"state.messages.length > {n_before} && state.messages[state.messages.length - 1].content.includes('(1-8)')", timeout=6000)
    bm.click("#messages .msg.ephemeral .eph-dismiss")
    bm.wait_for_selector("#messages .msg.ephemeral", state="detached")
    # A bot that never answers: the button spins, then "This interaction failed".
    bm.click("#messages .mc-btn:has-text('Wake up')")
    bm.wait_for_selector("#messages .mc-btn.loading .mc-spinner", timeout=3000)
    bm.screenshot(path=f"{SHOTS}/2504-desktop-component-loading.png")
    bm.wait_for_selector("#messages .mc-failed:has-text('This interaction failed')", timeout=6000)
    bm.screenshot(path=f"{SHOTS}/2505-desktop-interaction-failed.png")
    assert bm.get_attribute("#messages a.mc-btn.mc-link", "href") == "https://example.com/bots"
    bm.close()

    # Automod: a blocked message keeps your text; hidden ones get a "Filtered" cover.
    am_block = api("POST", f"/api/guilds/{BM}/channels", {"name": "automod-block"}, TOKEN)
    am_hide = api("POST", f"/api/guilds/{BM}/channels", {"name": "automod-hide"}, TOKEN)
    api("PUT", f"/api/guilds/{BM}/automod", {"words": ["gold seller*"]}, TOKEN)
    api("PUT", f"/api/channels/{am_block['id']}/automod", {"word_filter": True, "words": ["murloc"], "leetspeak": True,
        "spam_filter": True, "max_mentions": 3, "max_links": 2, "timeout_minutes": 5, "timeout_after": 10, "alert_channel_id": games["id"]}, TOKEN)
    api("PUT", f"/api/channels/{am_hide['id']}/automod", {"word_filter": True, "words": ["dang it"], "word_action": "hide"}, TOKEN)
    api("POST", f"/api/channels/{am_block['id']}/messages", {"content": "Automod's on in here, keep it clean."}, TOKEN)
    api("POST", f"/api/channels/{am_hide['id']}/messages", {"content": "Pulled too early, dang it"}, dave["token"])
    api("POST", f"/api/channels/{am_hide['id']}/messages", {"content": "All good, next pull in 30"}, erin["token"])
    for label, kw, shot in (("botsmod-dave", {"viewport": {"width": 1280, "height": 800}}, 2510),
                            ("botsmod-dave-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2511),
                            ("android-botsmod-dave", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2512)):
        dv = bm_page(dave["token"], label, am_block["id"], **kw)
        dv.fill("#composer-input", "selling MURL0C pets cheap")
        dv.keyboard.press("Enter")
        dv.wait_for_selector(".toast:has-text('Your message was blocked by automod')", timeout=5000)
        assert dv.evaluate("document.querySelector('#composer-input').value") == "selling MURL0C pets cheap"
        dv.screenshot(path=f"{SHOTS}/{shot}-{label}-automod-blocked-toast.png")
        dv.close()
    fh = bm_page(TOKEN, "botsmod-filtered", am_hide["id"], viewport={"width": 1280, "height": 800})
    fh.wait_for_selector("#messages .filtered-cover:has-text('Automod hid this message')", timeout=10000)
    assert not fh.query_selector("#messages .msg-text:has-text('dang it')")
    fh.screenshot(path=f"{SHOTS}/2513-desktop-filtered-cover.png")
    fh.click("#messages .filtered-cover .fc-show")
    fh.wait_for_selector("#messages .msg-text:has-text('dang it')")
    # Edit channel → Automod, and Outhouse settings → Automod.
    fh.evaluate(f"openEditChannel(chById({json.dumps(am_block['id'])}), 'automod')")
    fh.wait_for_selector("dialog.edit-channel .am-page .am-card", timeout=10000)
    fh.wait_for_timeout(300)
    assert fh.evaluate("document.querySelector('dialog.edit-channel .am-words').value") == "murloc"
    fh.screenshot(path=f"{SHOTS}/2514-desktop-automod-settings.png")
    fh.evaluate("document.querySelector('dialog.edit-channel .settings-main').scrollTop = 99999")
    fh.wait_for_timeout(200)
    fh.screenshot(path=f"{SHOTS}/2515-desktop-automod-settings-bottom.png")
    fh.check("dialog.edit-channel .am-role input")
    fh.wait_for_selector(".ec-savebar.show")
    fh.click(".ec-savebar button:has-text('Save changes')")
    fh.wait_for_selector(".ec-savebar.show", state="hidden", timeout=5000)
    assert len(api("GET", f"/api/channels/{am_block['id']}/automod", None, TOKEN)["exempt_roles"]) == 1
    fh.keyboard.press("Escape")
    fh.wait_for_selector("dialog.edit-channel", state="detached")
    fh.evaluate(f"openEditChannel(chById({json.dumps(games['id'])}), 'overview')")
    fh.wait_for_selector("dialog.edit-channel .am-thread-default select:not([disabled])", timeout=10000)
    fh.select_option("dialog.edit-channel .am-thread-default select", "1440")
    fh.wait_for_selector(".toast:has-text('New threads will use that')", timeout=5000)
    assert api("GET", f"/api/channels/{games['id']}/thread-defaults", None, TOKEN)["auto_archive_duration"] == 1440
    fh.keyboard.press("Escape")
    fh.wait_for_selector("dialog.edit-channel", state="detached")
    fh.evaluate("openSettings('automod')")
    fh.wait_for_selector("dialog.settings .am-defaults textarea", timeout=10000)
    assert "gold seller*" in fh.evaluate("document.querySelector('dialog.settings .am-defaults textarea').value")
    fh.screenshot(path=f"{SHOTS}/2516-desktop-outhouse-automod.png")
    fh.close()

    # Archived threads: hidden under a toggle, read-only banner with Unarchive.
    t_live = api("POST", f"/api/channels/{games['id']}/threads", {"name": "Bag rolls"}, dave["token"])
    api("POST", f"/api/channels/{games['id']}/messages", {"content": "I rolled 19 for the bag", "thread_id": t_live["id"]}, dave["token"])
    t_old = api("POST", f"/api/channels/{games['id']}/threads", {"name": "Last week's MC loot", "auto_archive_duration": 60}, dave["token"])
    api("POST", f"/api/channels/{games['id']}/messages", {"content": "Who got the Bindings?", "thread_id": t_old["id"]}, erin["token"])
    subprocess.run(["psql", os.environ["DATABASE_URL"], "-qc", f"UPDATE threads SET active_at = now() - interval '3 hours' WHERE id = {t_old['id']}"], check=True)
    for _ in range(60):
        if api("GET", f"/api/threads/{t_old['id']}", None, TOKEN)["archived"]:
            break
        time.sleep(0.5)
    else:
        raise AssertionError("the thread was never archived")
    for label, kw, shot in (("botsmod-threads", {"viewport": {"width": 1280, "height": 800}}, 2520),
                            ("botsmod-threads-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2521),
                            ("android-botsmod-threads", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2522)):
        tp = bm_page(erin["token"], label, games["id"], **kw)
        tp.click("#threads-btn")
        tp.wait_for_selector(".threads-panel .thread-card:has-text('Bag rolls')", timeout=10000)
        assert not tp.query_selector(".threads-panel .thread-card:has-text('MC loot')")
        tp.click(".threads-panel .archived-toggle")
        tp.wait_for_selector(".threads-panel .thread-card.archived:has-text('MC loot')")
        tp.wait_for_timeout(200)
        tp.screenshot(path=f"{SHOTS}/{shot}-{label}-archived-threads.png")
        tp.close()
    tp = bm_page(erin["token"], "botsmod-unarchive", games["id"], viewport={"width": 1280, "height": 800})
    tp.click("#threads-btn")
    tp.click(".threads-panel .archived-toggle")
    tp.click(".threads-panel .thread-card.archived")
    tp.wait_for_selector(".thread-archived-bar:has-text('archived')", timeout=10000)
    tp.wait_for_selector(".thread-panel .tmsg:has-text('Bindings')")
    tp.screenshot(path=f"{SHOTS}/2523-desktop-archived-thread-banner.png")
    tp.click(".thread-archived-bar .tab-unarchive")
    tp.wait_for_selector(".thread-archived-bar", state="detached", timeout=5000)
    assert api("GET", f"/api/threads/{t_old['id']}", None, TOKEN)["archived"] is False
    tp.close()

    # Phone and Android: bot buttons and the open dice menu, ephemeral reply, automod settings.
    for label, kw, shot in (("botsmod-phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2530),
                            ("android-botsmod", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2535)):
        mp = bm_page(TOKEN, label, games["id"], **kw)
        mp.wait_for_selector(f"{rollsel} .msg-components", timeout=10000)
        mp.evaluate(f"document.querySelector('{rollsel}').scrollIntoView({{block: 'center'}})")
        mp.wait_for_timeout(200)
        mp.screenshot(path=f"{SHOTS}/{shot}-{label}-bot-buttons.png")
        mp.click(f"{rollsel} .mc-btn:has-text('Roll in secret')")
        mp.wait_for_selector("#messages .msg.ephemeral", timeout=5000)
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{shot + 1}-{label}-ephemeral.png")
        mp.evaluate(f"document.querySelector('{rollsel}').scrollIntoView({{block: 'center'}})")
        mp.click(f"{rollsel} .mc-select")
        mp.wait_for_selector(".mc-pop .mc-option", timeout=5000)
        mp.wait_for_timeout(200)
        mp.screenshot(path=f"{SHOTS}/{shot + 2}-{label}-bot-select-open.png")
        mp.keyboard.press("Escape")
        mp.evaluate(f"openEditChannel(chById({json.dumps(am_block['id'])}), 'automod')")
        mp.wait_for_selector("dialog.edit-channel .am-page .am-card", timeout=10000)
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{shot + 3}-{label}-automod-settings.png")
        mp.close()
    dice_proc.terminate()
    sleepy_proc.terminate()

    # ---- Friends, voice messages, stickers, forwarding, scheduled messages ----
    soc = api("POST", f"/api/guilds/{G}/channels", {"name": "hangout"}, TOKEN)
    SOC = soc["id"]
    gwen = api("POST", "/api/auth/register", {"username": f"gwen{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Gwen"})
    grim = api("POST", "/api/auth/register", {"username": f"grim{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": "Grim"})
    api("POST", f"/api/invites/{code}", None, grim["token"])
    GRIM = grim["user"]["id"]
    # Dave and Carol are friends, Erin asked Carol, Carol asked Frank, Carol blocked Grim.
    api("PUT", f"/api/users/@me/relationships/{CAROL}", None, dave["token"])
    api("PUT", f"/api/users/@me/relationships/{DAVE}", None, TOKEN)
    api("PUT", f"/api/users/@me/relationships/{CAROL}", None, erin["token"])
    api("PUT", f"/api/users/@me/relationships/{FRANK}", None, TOKEN)
    api("POST", f"/api/channels/{SOC}/messages", {"content": "Anyone up for Strat UD later?"}, dave["token"])
    api("POST", f"/api/channels/{SOC}/messages", {"content": "lol you lot are terrible at this game"}, grim["token"])
    api("POST", f"/api/channels/{SOC}/messages", {"content": "and another thing about your DPS"}, grim["token"])
    api("PUT", f"/api/users/@me/blocks/{GRIM}", None, TOKEN)
    upload(f"/api/channels/{SOC}/messages/upload", {"content": "", "voice": json.dumps({"duration_ms": 2400, "waveform": "EDBQgK/f/9+vgFAwEDBQgK/f/9+vgA=="})},
           {"files": ("voice-message.wav", make_wav(2.4), "audio/wav")}, dave["token"])
    api("POST", f"/api/channels/{SOC}/stickers", {"sticker_id": "builtin:gg"}, dave["token"])
    big_png = make_png(96)
    stk = upload(f"/api/guilds/{G}/stickers", {"name": "Raid Smile", "emoji": "😄", "description": "Happy raiders"}, {"file": ("smile.png", big_png, "image/png")}, TOKEN)
    loot = api("POST", f"/api/channels/{general['id']}/messages", {"content": "Thunderfury dropped!! Who's the lucky one?"}, dave["token"])
    api("POST", f"/api/channels/{general['id']}/messages/{loot['id']}/forward", {"destinations": [SOC], "note": "Look what dropped in general"}, dave["token"])
    later = time.strftime("%Y-%m-%dT09:00:00Z", time.gmtime(time.time() + 3 * 86400))
    api("POST", f"/api/channels/{SOC}/scheduled", {"content": "Raid sign-ups close tonight, get in!", "send_at": later}, TOKEN)

    def social_page(token, label, **kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if label.startswith("android") else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(G)}); localStorage.setItem('outhouse_channel_{G}', {json.dumps(SOC)});")
        pg.goto(BASE + "/")
        pg.wait_for_selector("#messages .msg:has-text('Strat UD')", timeout=15000)
        pg.wait_for_selector("#messages .voice-msg", timeout=10000)
        return pg

    sp = social_page(TOKEN, "social-desktop", viewport={"width": 1280, "height": 800})
    # Grim's messages are folded away; Show brings them back.
    sp.wait_for_selector("#messages .blocked-bar:has-text('2 blocked messages')", timeout=10000)
    assert not sp.is_visible("#messages .msg:has-text('terrible at this game')")
    assert sp.is_visible("#messages .fwd .fwd-head:has-text('Forwarded from')"), "no forwarded message"
    assert "#general (UI Test Guild)" in sp.text_content("#messages .fwd .fwd-from")
    assert sp.is_visible("#messages .msg-sticker[src='/builtin-stickers/gg.svg']"), "no sticker"
    sp.wait_for_selector("#sched-bar:has-text('You have 1 scheduled message here')", timeout=10000)
    sp.wait_for_timeout(400)
    sp.screenshot(path=f"{SHOTS}/2200-desktop-social-channel.png")
    sp.click("#messages .blocked-bar .bb-show")
    sp.wait_for_selector("#messages .msg:has-text('terrible at this game')", timeout=5000)
    sp.screenshot(path=f"{SHOTS}/2201-desktop-blocked-shown.png")
    # Voice message player: play, speed.
    sp.click("#messages .voice-msg .vm-play")
    sp.wait_for_selector("#messages .voice-msg.playing", timeout=5000)
    sp.click("#messages .voice-msg .vm-rate")
    assert sp.text_content("#messages .voice-msg .vm-rate") == "1.5×"
    sp.wait_for_timeout(700)
    sp.screenshot(path=f"{SHOTS}/2202-desktop-voice-player.png")
    sp.click("#messages .voice-msg .vm-play")
    # Record one (Chrome's fake microphone beeps).
    in_voice_before = sp.evaluate("!!micPipe.stream")
    sp.click("#mic-btn")
    sp.wait_for_selector("#composer .vm-recbar", timeout=10000)
    sp.wait_for_timeout(2600)
    sp.screenshot(path=f"{SHOTS}/2203-desktop-voice-recording.png")
    sp.click("#composer .vm-recbar .vm-send")
    sp.wait_for_function("state.messages.some((m) => m.author_id === state.me.id && m.attachments.some((a) => a.voice && a.duration_ms >= 1500))", timeout=15000)
    assert sp.evaluate("!!micPipe.stream") == in_voice_before, "the voice channel's mic pipeline was touched"
    voice_att = sp.evaluate("state.messages.filter((m) => m.author_id === state.me.id).pop().attachments[0]")
    print("recorded voice message:", {k: voice_att[k] for k in ("content_type", "duration_ms", "size")}, "waveform:", bool(voice_att.get("waveform")))
    assert voice_att["content_type"] in ("audio/webm", "audio/ogg", "audio/mp4"), voice_att
    sp.wait_for_timeout(400)
    sp.screenshot(path=f"{SHOTS}/2204-desktop-voice-sent.png")
    # Recording then binning it sends nothing.
    count = sp.evaluate("state.messages.length")
    sp.click("#mic-btn")
    sp.wait_for_selector("#composer .vm-recbar", timeout=10000)
    sp.wait_for_timeout(900)
    sp.click("#composer .vm-recbar .vm-cancel")
    sp.wait_for_selector("#composer .vm-recbar", state="detached")
    sp.wait_for_timeout(1000)
    assert sp.evaluate("state.messages.length") == count
    # Stickers: the third tab of the emoji picker.
    sp.click("#emoji-btn")
    sp.click("#sticker-tab")
    sp.wait_for_selector(".sticker-pane .sticker-tile[data-sticker='builtin:occupied']", timeout=5000)
    sp.wait_for_selector(f".sticker-pane .sticker-tile[data-sticker='{stk['id']}']", timeout=5000)
    sp.wait_for_timeout(400)
    sp.screenshot(path=f"{SHOTS}/2205-desktop-sticker-picker.png")
    sp.fill(".sticker-pane .sticker-search", "vacant")
    assert sp.eval_on_selector_all(".sticker-pane .sticker-tile", "els => els.length") == 1
    sp.fill(".sticker-pane .sticker-search", "")
    sp.click(f".sticker-pane .sticker-tile[data-sticker='{stk['id']}']")
    sp.wait_for_selector(f"#messages .msg-sticker[src='/stickers/{stk['id']}']", timeout=10000)
    sp.wait_for_timeout(500)
    sp.screenshot(path=f"{SHOTS}/2206-desktop-sticker-sent.png")
    # Forward Dave's question to Dave's DM and #general, with a note.
    q_id = sp.evaluate("state.messages.find((m) => m.content.includes('Strat UD')).id")
    sp.click(f"#msg-{q_id}", button="right")
    sp.click(".menu button:has-text('Forward')")
    sp.wait_for_selector(".fwd-dialog[open] .fwd-row", timeout=10000)
    sp.wait_for_selector(f".fwd-dialog input[data-target='{general['id']}']")
    sp.fill(".fwd-dialog input[type=search]", "general")
    sp.check(f".fwd-dialog input[data-target='{general['id']}']")
    sp.fill(".fwd-dialog input[type=search]", "")
    dave_dm = api("POST", "/api/dms", {"user_ids": [DAVE]}, TOKEN)["id"]
    sp.check(f".fwd-dialog input[data-target='{dave_dm}']")
    sp.fill(".fwd-dialog .fwd-note", "You keen, Dave?")
    sp.wait_for_timeout(300)
    sp.screenshot(path=f"{SHOTS}/2207-desktop-forward-dialog.png")
    sp.click(".fwd-dialog button[type=submit]")
    sp.wait_for_selector(".fwd-dialog", state="detached", timeout=10000)
    fwd_msgs = api("GET", f"/api/channels/{dave_dm}/messages?limit=5", None, dave["token"])
    assert any(m.get("forward") and m["content"] == "You keen, Dave?" and m["forward"]["channel_name"] == "#hangout" for m in fwd_msgs), fwd_msgs
    # Scheduled messages: the ▾ next to Send.
    sp.fill("#composer-input", "Don't forget repair bills, raid in an hour")
    sp.click("#send-later")
    sp.click(".menu button:has-text('Send later')")
    sp.wait_for_selector(".sched-dialog[open] .sched-preset", timeout=5000)
    sp.click(".sched-dialog .sched-preset[data-choice='morning']")
    sp.wait_for_timeout(300)
    sp.screenshot(path=f"{SHOTS}/2208-desktop-send-later.png")
    sp.click(".sched-dialog button[type=submit]")
    sp.wait_for_selector("#sched-bar:has-text('You have 2 scheduled messages here')", timeout=10000)
    assert sp.input_value("#composer-input") == ""
    sp.screenshot(path=f"{SHOTS}/2209-desktop-scheduled-bar.png")
    sp.click("#sched-bar")
    sp.wait_for_selector(".sched-list .sched-item >> nth=1", timeout=5000)
    sp.wait_for_timeout(300)
    sp.screenshot(path=f"{SHOTS}/2210-desktop-scheduled-list.png")
    sp.click(".sched-item:has-text('repair bills') button:has-text('Send now')")
    sp.wait_for_selector("#messages .msg:has-text('repair bills')", timeout=10000)
    sp.click(".sched-item:has-text('sign-ups close') button:has-text('Cancel')")
    sp.wait_for_selector(".sched-list p:has-text('Nothing scheduled here')", timeout=5000)
    sp.keyboard.press("Escape")
    sp.wait_for_selector("#sched-bar", state="detached", timeout=5000)
    # Home → Friends.
    sp.click("#home-btn")
    sp.wait_for_selector("#friends-row", timeout=10000)
    sp.wait_for_selector("#home-btn .mention-badge", timeout=10000)  # Erin's request
    sp.click("#friends-row")
    sp.wait_for_selector(".friends-page .friend-tabs", timeout=5000)
    assert sp.is_hidden("#composer-wrap")
    sp.click(".ftab[data-tab='all']")
    sp.wait_for_selector(".friend-row:has-text('Dave')")
    sp.screenshot(path=f"{SHOTS}/2211-desktop-friends-all.png")
    sp.click(".ftab[data-tab='pending']")
    sp.wait_for_selector(".friend-row.fr-incoming:has-text('Erin')")
    sp.wait_for_selector(".friend-row.fr-outgoing:has-text('Frank')")
    sp.screenshot(path=f"{SHOTS}/2212-desktop-friends-pending.png")
    sp.click(".friend-row.fr-incoming:has-text('Erin') .fr-btn.ok")
    sp.wait_for_selector(".friend-row.fr-incoming", state="detached", timeout=10000)
    sp.click(".ftab[data-tab='blocked']")
    sp.wait_for_selector(".friend-row.fr-blocked:has-text('Grim')")
    sp.click(".ftab[data-tab='add']")
    sp.fill("#add-friend-input", f"gwen{suffix}")
    sp.click(".add-friend button[type=submit]")
    sp.wait_for_selector(".af-status.ok:has-text('Gwen')", timeout=10000)
    sp.fill("#add-friend-input", "nobody-by-this-name")
    sp.click(".add-friend button[type=submit]")
    sp.wait_for_selector(".af-status.err", timeout=10000)
    sp.screenshot(path=f"{SHOTS}/2213-desktop-add-friend.png")
    # A live friend request (gateway) bumps the Pending count.
    api("PUT", f"/api/users/@me/relationships/{CAROL}", None, gwen["token"])  # Gwen accepts Carol's
    sp.click(".ftab[data-tab='all']")
    sp.wait_for_selector(".friend-row:has-text('Gwen')", timeout=10000)
    # Friend buttons on profile cards and the member list menu.
    sp.click(".guild[title='UI Test Guild']")
    sp.wait_for_selector("#messages .msg:has-text('Strat UD')", timeout=10000)
    sp.click(f"#member-list .member[data-id='{FRANK}'] .avatar-wrap")
    sp.wait_for_selector(".profile-card .pc-friend.fr-outgoing:has-text('Request sent')", timeout=5000)
    sp.screenshot(path=f"{SHOTS}/2214-desktop-profile-friend.png")
    sp.keyboard.press("Escape")
    sp.click(f"#member-list .member[data-id='{FRANK}']", button="right")
    sp.wait_for_selector(".menu button:has-text('Cancel friend request')")
    sp.wait_for_selector(".menu button:has-text('Block')")
    sp.keyboard.press("Escape")
    # Settings pages.
    sp.evaluate("openSettings('stickers')")
    sp.wait_for_selector(".sticker-admin-grid .sticker-card:has-text('Raid Smile')", timeout=10000)
    sp.screenshot(path=f"{SHOTS}/2215-desktop-sticker-settings.png")
    sp.keyboard.press("Escape")
    sp.evaluate("openUserSettings('privacy')")
    sp.wait_for_selector(".settings-main h2:has-text('Privacy & friends')", timeout=10000)
    sp.wait_for_selector(".settings-main .friend-row:has-text('Grim')")
    sp.screenshot(path=f"{SHOTS}/2216-desktop-privacy.png")
    sp.click("input[name=allow_dms][value=friends]")
    sp.wait_for_timeout(600)
    assert api("GET", "/api/users/@me/privacy", None, TOKEN)["allow_dms"] == "friends"
    api("PUT", "/api/users/@me/privacy", {"allow_dms": "everyone"}, TOKEN)
    sp.keyboard.press("Escape")
    sp.close()

    for label, kw, n in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 2220),
                         ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 2240)):
        mp = social_page(TOKEN, f"{label}-social", **kw)
        mp.wait_for_timeout(500)
        mp.screenshot(path=f"{SHOTS}/{n}-{label}-social-channel.png")
        # Press and hold the mic, let go to send.
        box = mp.query_selector("#mic-btn").bounding_box()
        cx, cy = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        ptr = {"pointerType": "touch", "pointerId": 7, "isPrimary": True, "clientX": cx, "clientY": cy, "bubbles": True}
        mp.dispatch_event("#mic-btn", "pointerdown", ptr)
        mp.wait_for_selector("#composer .vm-recbar.hold", timeout=10000)
        mp.wait_for_timeout(1800)
        mp.screenshot(path=f"{SHOTS}/{n + 1}-{label}-voice-hold.png")
        mp.dispatch_event("#mic-btn", "pointermove", {**ptr, "clientX": cx - 120})
        mp.wait_for_selector("#composer .vm-recbar.cancel", timeout=5000)
        mp.screenshot(path=f"{SHOTS}/{n + 2}-{label}-voice-slide-cancel.png")
        mp.dispatch_event("#mic-btn", "pointermove", {**ptr})
        mp.dispatch_event("#mic-btn", "pointerup", ptr)
        mp.wait_for_selector("#composer .vm-recbar", state="detached", timeout=5000)
        mp.wait_for_function("state.messages.filter((m) => m.author_id === state.me.id && m.attachments.some((a) => a.voice)).length >= 2", timeout=15000)
        mp.wait_for_timeout(500)
        mp.screenshot(path=f"{SHOTS}/{n + 3}-{label}-voice-sent.png")
        # Long-press Send for "Send later".
        mp.fill("#composer-input", "Raid at 8, don't be late")
        sbox = mp.query_selector("#composer .send").bounding_box()
        sptr = {"pointerType": "touch", "pointerId": 8, "isPrimary": True, "clientX": sbox["x"] + 10, "clientY": sbox["y"] + 10, "bubbles": True}
        mp.dispatch_event("#composer .send", "pointerdown", sptr)
        mp.wait_for_selector(".menu button:has-text('Send later')", timeout=5000)
        mp.dispatch_event("#composer .send", "pointerup", sptr)
        mp.screenshot(path=f"{SHOTS}/{n + 4}-{label}-send-later-menu.png")
        mp.click(".menu button:has-text('Send later')")
        mp.wait_for_selector(".sched-dialog[open] .sched-preset")
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{n + 5}-{label}-send-later.png")
        mp.click(".sched-dialog .sched-preset[data-choice='hour']")
        mp.click(".sched-dialog button[type=submit]")
        mp.wait_for_selector("#sched-bar", timeout=10000)
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{n + 6}-{label}-scheduled-bar.png")
        for s in api("GET", "/api/users/@me/scheduled", None, TOKEN):
            api("DELETE", f"/api/scheduled/{s['id']}", None, TOKEN)
        # Sticker picker.
        mp.click("#emoji-btn")
        mp.click("#sticker-tab")
        mp.wait_for_selector(".sticker-pane .sticker-tile", timeout=5000)
        mp.wait_for_timeout(400)
        mp.screenshot(path=f"{SHOTS}/{n + 7}-{label}-sticker-picker.png")
        mp.keyboard.press("Escape")
        # Forward dialog.
        fid = mp.evaluate("state.messages.find((m) => m.forward).id")
        mp.evaluate(f"openForwardDialog(findMessage({json.dumps(fid)}))")
        mp.wait_for_selector(".fwd-dialog[open] .fwd-row", timeout=10000)
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{n + 8}-{label}-forward-dialog.png")
        mp.keyboard.press("Escape")
        # Home → Friends.
        mp.click("#nav-toggle")
        mp.wait_for_timeout(400)
        mp.click("#home-btn")
        mp.wait_for_selector("#friends-row", timeout=10000)
        if not mp.evaluate("document.getElementById('app').classList.contains('show-nav')"):
            mp.click("#nav-toggle")
            mp.wait_for_timeout(400)
        mp.click("#friends-row")
        mp.wait_for_selector(".friends-page .friend-tabs", timeout=5000)
        mp.wait_for_timeout(400)
        mp.click(".ftab[data-tab='all']")
        mp.wait_for_selector(".friend-row:has-text('Dave')")
        mp.screenshot(path=f"{SHOTS}/{n + 9}-{label}-friends.png")
        mp.click(".ftab[data-tab='add']")
        mp.wait_for_timeout(300)
        mp.screenshot(path=f"{SHOTS}/{n + 10}-{label}-add-friend.png")
        mp.close()


    # ---- The raid sign-up card: a full 25-man roster, a 40-man, an empty one ----
    def api_from(ip, method, path, body=None, token=None):
        """Like api(), as if from another address (sign-ups are limited per address)."""
        req = urllib.request.Request(BASE + path, method=method)
        req.add_header("Content-Type", "application/json")
        req.add_header("X-Forwarded-For", ip)
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        data = json.dumps(body).encode() if body is not None else None
        with urllib.request.urlopen(req, data) as res:
            text = res.read().decode()
            return json.loads(text) if text else None

    rg = api("POST", "/api/guilds", {"name": "Raid Card Hall"}, TOKEN)
    rchan = api("POST", f"/api/guilds/{rg['id']}/channels", {"name": "raid-signups"}, TOKEN)
    rcode = api("POST", f"/api/guilds/{rg['id']}/invites", None, TOKEN)["code"]
    # (name, character "First Last" or None, class, spec, role)
    RC_PEOPLE = [
        ("Golgoroth", "Golgoroth Ironmaw", "Warrior", "Protection", "tank"),
        ("Brisk", None, "Warrior", "Protection", "tank"),
        ("Mossheart", "Mossheart Thornfield", "Druid", "Feral (Bear)", "tank"),
        ("Kaelith", None, "Priest", "Holy", "healer"),
        ("Gorvash", "Gorvash Emberfist", "Priest", "Discipline", "healer"),
        ("Lunessa", None, "Paladin", "Holy", "healer"),
        ("Ironjaw", None, "Paladin", "Holy", "healer"),
        ("Pyrella", "Pyrella Dawnbrook", "Druid", "Restoration", "healer"),
        ("Tidecall", None, "Shaman", "Restoration", "healer"),
        ("Shadowmend", None, "Warrior", "Fury", "dps"),
        ("Vexx", "Vexx Nightbloom", "Warrior", "Arms", "dps"),
        ("Hollowpine", None, "Rogue", "Combat", "dps"),
        ("Brakka", None, "Rogue", "Combat", "dps"),
        ("Seraphine", "Seraphine Vale", "Rogue", "Assassination", "dps"),
        ("Duskwhisper", None, "Mage", "Frost", "dps"),
        ("Grimtooth", None, "Mage", "Fire", "dps"),
        ("Emberly", "Emberly Ashgrove", "Mage", "Arcane", "dps"),
        ("Frostnova", None, "Warlock", "Destruction", "dps"),
        ("Quillon", None, "Warlock", "Affliction", "dps"),
        ("Ragna", "Ragna Stormhide", "Hunter", "Marksmanship", "dps"),
        ("Stoneroot", None, "Hunter", "Survival", "dps"),
        ("Velaria", None, "Shaman", "Enhancement", "dps"),
        ("Zuljin", None, "Priest", "Shadow", "dps"),
        ("Ashka", None, "Paladin", "Retribution", "dps"),
        ("Morrow", None, "Druid", "Balance", "dps"),
        ("Nettle", None, "Rogue", "Combat", "dps"),
        ("Oakhide", None, "Warrior", "Fury", "dps"),
        ("Pellwyn", None, "Mage", "Frost", "dps"),
        ("Rook", None, "Hunter", "Beast Mastery", "dps"),
        ("Sable", None, "Warlock", "Demonology", "dps"),
        ("Tamsin", None, "Priest", "Holy", "healer"),
        ("Wren", None, "Shaman", "Restoration", "healer"),
    ]
    rc_users = []
    for i, (name, char, cls, spec, role) in enumerate(RC_PEOPLE):
        ip = f"198.18.{1 + i // 12}.{10 + i}"
        u = api_from(ip, "POST", "/api/auth/register", {"username": f"rc{i}x{suffix}", "password": "password123", "birth_date": "1990-01-01", "display_name": name})
        api_from(ip, "POST", f"/api/invites/{rcode}", None, u["token"])
        cid = None
        if char:
            first, last = char.split(" ", 1)
            cid = api("POST", "/api/users/@me/characters", {"name": first, "last_name": last, "class": cls, "level": 60}, u["token"])["id"]
        rc_users.append({"token": u["token"], "id": u["user"]["id"], "cls": cls, "spec": spec, "role": role, "char": cid})
    for t in (dave["token"], erin["token"]):
        api("POST", f"/api/invites/{rcode}", None, t)

    def rc_sign(ev_id, u, status="yes"):
        body = {"status": status, "role": u["role"], "class": u["cls"], "spec": u["spec"]}
        if u["char"]:
            body["character_id"] = u["char"]
        api("PUT", f"/api/events/{ev_id}/signup", body, u["token"])

    later = lambda days: time.strftime("%Y-%m-%dT19:30:00Z", time.gmtime(time.time() + days * 86400))
    # 25-man with soft reserves: 25 on the roster (one late, two tentative),
    # two benched when DPS filled up, three can't make it; about 2/3 reserved.
    r25 = api("POST", f"/api/channels/{rchan['id']}/events", {
        "title": "Molten Core: full clear", "description": "Fire resist for Rag, flasks and food. Meet at the Blackrock Mountain stone 15 minutes early.\nVent: Raid comms. Soft reserves 2 each, lock when sign-ups close.",
        "starts_at": later(3), "duration_minutes": 180, "game": "forever", "max_tanks": 3, "max_healers": 6, "max_dps": 16,
        "soft_reserves": 2, "reserve_lock": "deadline", "loot_instance": "mc", "signup_deadline_minutes": 120,
    }, TOKEN)["event"]
    R25 = r25["id"]
    for i, u in enumerate(rc_users[:25]):
        rc_sign(R25, u, "late" if i == 6 else "tentative" if i == 13 else "yes")
    for u in rc_users[25:28]:                   # the last DPS spot, then two on the bench
        rc_sign(R25, u)
    rc_sign(R25, rc_users[28], "tentative")
    for u in rc_users[29:32]:
        rc_sign(R25, u, "no")
    items = [17204, 18814, 17076, 17106, 16922, 17069, 17103, 18842]
    for i, u in enumerate(rc_users[:28]):
        if i % 3 == 2:
            continue
        try:
            api("PUT", f"/api/events/{R25}/reserves", {"item_id": items[i % len(items)]}, u["token"])
        except urllib.error.HTTPError as e:
            print("reserve skipped:", e.code, e.read()[:200])
    # 40-man: ~30 across all nine classes, two benched by the leader, three absences.
    r40 = api("POST", f"/api/channels/{rchan['id']}/events", {
        "title": "Naxxramas 40", "description": "Spider and Plague wings.", "starts_at": later(5), "duration_minutes": 240,
        "game": "forever", "max_tanks": 5, "max_healers": 10, "max_dps": 25,
    }, TOKEN)["event"]
    R40 = r40["id"]
    for u in rc_users[:29]:
        rc_sign(R40, u)
    api("PUT", f"/api/events/{R40}/signup", {"status": "yes", "role": "healer", "class": "Priest", "spec": "Holy"}, TOKEN)
    for u in rc_users[29:31]:
        api("PUT", f"/api/events/{R40}/signup", {"user_id": u["id"], "status": "bench", "role": u["role"], "class": u["cls"], "spec": u["spec"]}, TOKEN)
    for t in (dave["token"], erin["token"]):
        api("PUT", f"/api/events/{R40}/signup", {"status": "no"}, t)
    rc_sign(R40, rc_users[31], "no")
    # A brand-new 10-man nobody has signed up to yet.
    r10 = api("POST", f"/api/channels/{rchan['id']}/events", {
        "title": "Zul'Gurub", "description": "Quick run for the Edge of Madness.", "starts_at": later(6),
        "game": "forever", "max_tanks": 2, "max_healers": 3, "max_dps": 5,
    }, TOKEN)["event"]
    R10 = r10["id"]

    def rc_page(token, label, **kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if label.startswith("android") else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(rg['id'])}); localStorage.setItem('outhouse_channel_{rg['id']}', {json.dumps(rchan['id'])});")
        pg.goto(BASE + "/")
        pg.wait_for_selector(f".event-card[data-event='{R25}'] .rc-roster", timeout=15000)
        return pg

    def rc_card(pg, ev_id):
        sel = f"#messages .event-card[data-event='{ev_id}']"
        pg.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", sel)
        pg.wait_for_timeout(300)
        return sel

    member = rc_users[3]  # Kaelith, an ordinary raider
    # Desktop, as a raider sees it.
    dp = rc_page(member["token"], "raidcard-desktop", viewport={"width": 1280, "height": 900})
    dp.set_viewport_size({"width": 1280, "height": 1600})
    c25 = rc_card(dp, R25)
    assert dp.inner_text(f"{c25} .rc-count") == "25/25 (+2)", dp.inner_text(f"{c25} .rc-count")
    assert dp.query_selector_all(f"{c25} .rc-class").__len__() == 9
    dp.wait_for_selector(f"{c25} .rc-entry.st-late .rc-mark.late")
    dp.wait_for_selector(f"{c25} .rc-entry.st-tentative .rc-mark.maybe")
    dp.wait_for_selector(f"{c25} .rc-class[data-class='Warrior'] .rc-class-head:has-text('Warrior')")
    dp.wait_for_selector(f"{c25} .su-name:has-text('Golgoroth Ironmaw')")
    assert "Golgoroth" in dp.get_attribute(f"{c25} .su-name:has-text('Golgoroth Ironmaw')", "title")
    dp.wait_for_selector(f"{c25} .ev-bench:has-text('Bench (2)')")
    dp.wait_for_selector(f"{c25} .ev-absent:has-text('Absence (3)')")
    dp.wait_for_selector(f"{c25} .rc-roles .rc-role.ranged")
    n_sr = len(dp.query_selector_all(f"{c25} .rc-roster .rc-sr:not(.none)"))
    assert 12 <= n_sr <= 22, n_sr
    assert not dp.query_selector(f"{c25} .rc-sr.none"), "only raid leaders see who hasn't reserved"
    assert not dp.query_selector(f"{c25} .ev-gaps"), "the gaps chip is for raid leaders"
    assert not dp.query_selector(f"{c25} button[title='Raid options']")
    dp.screenshot(path=f"{SHOTS}/3300-desktop-raidcard-25.png")
    dp.set_viewport_size({"width": 1280, "height": 1600})
    c25 = rc_card(dp, R25)
    dp.locator(c25).screenshot(path=f"{SHOTS}/3301-raidcard-25-card.png")
    dp.set_viewport_size({"width": 1280, "height": 900})
    c25 = rc_card(dp, R25)
    # The status menu (late / tentative / can't make it / withdraw).
    dp.click(f"{c25} .rc-status")
    dp.wait_for_selector(".menu button:has-text('Tentative')")
    dp.wait_for_selector(".menu button:has-text('Withdraw')")
    dp.screenshot(path=f"{SHOTS}/3302-desktop-raidcard-status-menu.png")
    dp.click(".menu button:has-text('Running late')")
    dp.wait_for_selector(".signup-pop")
    dp.click(".signup-pop button.primary")
    dp.wait_for_selector(f"{c25} .rc-entry[data-user='{member['id']}'].st-late", timeout=10000)
    dp.wait_for_selector(f"{c25} .rc-status:has-text('Running late')")
    # Soft reserves open in their own window.
    dp.click(f"{c25} .ev-link.sr")
    dp.wait_for_selector("dialog.loot-dialog[open]", timeout=10000)
    dp.screenshot(path=f"{SHOTS}/3303-desktop-raidcard-sr-window.png")
    dp.click("dialog.loot-dialog .raid-close")
    dp.set_viewport_size({"width": 1280, "height": 1600})
    c40 = rc_card(dp, R40)
    assert dp.inner_text(f"{c40} .rc-count") == "30/40 (+2)", dp.inner_text(f"{c40} .rc-count")
    dp.locator(c40).screenshot(path=f"{SHOTS}/3304-raidcard-40-card.png")
    c10 = rc_card(dp, R10)
    dp.wait_for_selector(f"{c10} .rc-empty")
    dp.locator(c10).screenshot(path=f"{SHOTS}/3305-raidcard-empty-card.png")
    # The calendar's day panel reuses the card.
    dp.set_viewport_size({"width": 1280, "height": 900})
    dp.evaluate(f"openCalendar(new Date({json.dumps(later(3))}))")
    dp.wait_for_selector(f".calendar-dialog .cal-daypanel .event-card[data-event='{R25}'] .rc-roster", timeout=10000)
    dp.wait_for_timeout(300)
    dp.screenshot(path=f"{SHOTS}/3306-desktop-calendar-raidcard.png")
    dp.close()

    # The raid leader: the gaps chip, then the composition it opens.
    ld = rc_page(TOKEN, "raidcard-leader", viewport={"width": 1280, "height": 2000})
    c25 = rc_card(ld, R25)
    ld.wait_for_selector(f"{c25} .rc-sr.none")  # leaders see who hasn't reserved yet
    ld.wait_for_selector(f"{c25} .ev-link.export:has-text('Invite list')")
    ld.wait_for_selector(f"{c25} .ev-gaps:not([hidden]), {c25} .ev-comp-ok:not([hidden])", timeout=10000)
    ld.locator(c25).screenshot(path=f"{SHOTS}/3307-raidcard-leader-card.png")
    # The 40-man is short of tanks and healers: a gaps chip that opens the composition.
    c40 = rc_card(ld, R40)
    ld.wait_for_selector(f"{c40} .ev-gaps:not([hidden])", timeout=10000)
    need40 = api("GET", f"/api/events/{R40}/composition", None, TOKEN)["missing_line"]
    assert ld.get_attribute(f"{c40} .ev-gaps", "title") == need40
    ld.locator(c40).screenshot(path=f"{SHOTS}/3308-raidcard-leader-gaps.png")
    ld.click(f"{c40} .ev-gaps")
    ld.wait_for_selector(f"{c40} details.ev-comp[open] .comp-missing", timeout=10000)
    ld.wait_for_timeout(400)
    ld.locator(c40).screenshot(path=f"{SHOTS}/3309-raidcard-leader-comp-open.png")
    # Keyboard: a leader can open a roster name's menu with Enter.
    ld.focus(f"{c25} .rc-entry .su-name.manageable >> nth=2")
    ld.keyboard.press("Enter")
    ld.wait_for_selector(".menu button:has-text('Move to bench')")
    ld.keyboard.press("Escape")
    ld.close()

    # Phone and Android widths.
    for label, kw, shot in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True, "device_scale_factor": 2}, 3310),
                            ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True, "device_scale_factor": 2}, 3312)):
        mp = rc_page(member["token"], f"raidcard-{label}", **kw)
        c25 = rc_card(mp, R25)
        mp.screenshot(path=f"{SHOTS}/{shot}-{label}-raidcard-25.png")
        mp.wait_for_selector(f"{c25} .su-name:has-text('Golgoroth') .nm-last", state="hidden")  # narrow: first names only
        mp.set_viewport_size({"width": kw["viewport"]["width"], "height": 2600})
        c25 = rc_card(mp, R25)
        mp.locator(c25).screenshot(path=f"{SHOTS}/{shot + 1}-{label}-raidcard-25-card.png")
        mp.close()

    # ---- Raid composition, soft reserves and the loot log (loot.js) ----
    lg = api("POST", "/api/guilds", {"name": "Loot Hall"}, TOKEN)
    lchan = api("POST", f"/api/guilds/{lg['id']}/channels", {"name": "raid-loot"}, TOKEN)
    lcode = api("POST", f"/api/guilds/{lg['id']}/invites", None, TOKEN)["code"]
    for t in (dave["token"], erin["token"]):
        api("POST", f"/api/invites/{lcode}", None, t)
    sr_raid = api("POST", f"/api/channels/{lchan['id']}/events", {
        "title": "Molten Core reserves", "starts_at": time.strftime("%Y-%m-%dT20:00:00Z", time.gmtime(time.time() + 3 * 86400)),
        "game": "forever", "max_tanks": 1, "max_healers": 2, "max_dps": 3,
        "soft_reserves": 2, "reserve_lock": "manual", "loot_instance": "mc",
    }, TOKEN)["event"]
    SRID = sr_raid["id"]
    api("PUT", f"/api/events/{SRID}/signup", {"status": "yes", "role": "tank", "class": "Warrior", "spec": "Protection"}, TOKEN)
    api("PUT", f"/api/events/{SRID}/signup", {"status": "yes", "role": "healer", "class": "Priest", "spec": "Holy"}, dave["token"])
    api("PUT", f"/api/events/{SRID}/signup", {"status": "tentative", "role": "dps", "class": "Mage", "spec": "Frost"}, erin["token"])
    api("PUT", f"/api/events/{SRID}/reserves", {"item_id": 17076}, dave["token"])
    api("PUT", f"/api/events/{SRID}/reserves", {"item_id": 17076}, erin["token"])
    api("PUT", f"/api/events/{SRID}/reserves", {"item_id": 18814}, erin["token"])
    api("PUT", f"/api/events/{SRID}/reserves", {"item_id": 16922}, TOKEN)
    past_raid = api("POST", f"/api/channels/{lchan['id']}/events", {
        "title": "Onyxia last night", "starts_at": iso(time.time() - 30 * 60), "max_tanks": 1, "max_healers": 2, "max_dps": 5,
        "soft_reserves": 1, "reserve_lock": "manual", "loot_instance": "ony",
    }, TOKEN)["event"]
    PRID = past_raid["id"]
    api("PUT", f"/api/events/{PRID}/signup", {"status": "yes", "role": "healer", "class": "Priest", "spec": "Holy"}, dave["token"])
    api("PUT", f"/api/events/{PRID}/signup", {"status": "yes", "role": "dps", "class": "Mage", "spec": "Fire"}, erin["token"])
    api("PUT", f"/api/events/{PRID}/reserves", {"item_id": 17078}, dave["token"])
    api("POST", f"/api/events/{PRID}/loot", {"user_id": DAVE, "item_id": 17078}, TOKEN)
    api("POST", f"/api/events/{PRID}/loot", {"user_id": ERIN, "item_id": 18205, "note": "off-spec"}, TOKEN)
    need_line = api("GET", f"/api/events/{SRID}/composition", None, TOKEN)["missing_line"]
    assert need_line.startswith("Need: 1 healer · 3 DPS"), need_line

    def loot_page(token, label, **kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        extra = "window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = 'android'; window.__OUTHOUSE_APP_VERSION__ = '0.2.1';" if label.startswith("android") else ""
        pg.add_init_script(extra + f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(lg['id'])}); localStorage.setItem('outhouse_channel_{lg['id']}', {json.dumps(lchan['id'])});")
        pg.goto(BASE + "/")
        pg.wait_for_selector(".event-card .ev-link.sr", timeout=15000)
        return pg

    def sr_card(pg):
        scroll_to_card(pg, "Molten Core reserves")
        return f".event-card[data-event='{SRID}']"

    lp = loot_page(TOKEN, "loot-desktop", viewport={"width": 1280, "height": 900})
    card = sr_card(lp)
    # Raid leaders get a small "gaps" chip; its tooltip is what the server works out.
    lp.wait_for_selector(f"{card} .ev-gaps:not([hidden])", timeout=10000)
    shown = lp.get_attribute(f"{card} .ev-gaps", "title")
    assert shown == need_line, (shown, need_line)
    assert "gaps" in lp.inner_text(f"{card} .ev-gaps")
    lp.wait_for_selector(f"{card} .ev-link.sr[title*='2 each']")
    lp.wait_for_selector(f"{card} .ev-link.sr:has-text('Soft reserves (4)')")
    # Who has reserved shows as an SR tag on the roster; the items stay in the window.
    lp.wait_for_selector(f"{card} .rc-entry[data-user='{CAROL}'] .rc-sr:not(.none)")
    lp.wait_for_selector(f"{card} .rc-entry[data-user='{ERIN}'].st-tentative .rc-sr:not(.none)")
    assert not lp.query_selector(f"{card} .item-name"), "reserved items shouldn't be on the card"
    lp.screenshot(path=f"{SHOTS}/150-desktop-raid-card-loot.png")
    # Composition: Erin's tentative Mage doesn't count until you ask.
    # The gaps chip opens the composition, which spells out what's needed.
    lp.click(f"{card} .ev-gaps")
    lp.wait_for_selector(f"{card} .comp-buff[data-buff='fortitude'].yes", timeout=10000)
    assert need_line.split("Need: ", 1)[1] in lp.inner_text(f"{card} .comp-missing")
    lp.wait_for_selector(f"{card} .comp-buff[data-buff='arcane_intellect'].no")
    lp.wait_for_selector(f"{card} .comp-role.short:has-text('Healers 1/2')")
    assert "Dave" in lp.get_attribute(f"{card} .comp-buff[data-buff='fortitude']", "title")
    lp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{card} .ev-comp")
    lp.wait_for_timeout(250)
    lp.screenshot(path=f"{SHOTS}/151-desktop-composition.png")
    lp.click(f"{card} .comp-extra input")
    lp.wait_for_selector(f"{card} .comp-buff[data-buff='arcane_intellect'].yes", timeout=10000)
    lp.wait_for_selector(f"{card} .comp-buff[data-buff='decurse'].yes")
    lp.wait_for_selector(f"{card} .ev-comp-sum:has-text('incl. tentative')")
    lp.hover(f"{card} .comp-buff[data-buff='winters_chill']")
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/152-desktop-composition-with-tentative.png")
    # The raid leader's reserves table.
    lp.click(f"{card} .ev-link.sr")
    lp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='17076'].contested", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='17076'] .sr-count:has-text('2 SR')")
    lp.wait_for_selector("dialog.loot-dialog .loot-tab.on:has-text(\"Everyone's (4)\")")
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/153-desktop-sr-table-by-item.png")
    lp.click("dialog.loot-dialog .seg-btn:has-text('By player')")
    lp.wait_for_selector(f"dialog.loot-dialog .sr-trow[data-user='{ERIN}'] .sr-chip >> nth=1")
    lp.screenshot(path=f"{SHOTS}/154-desktop-sr-table-by-player.png")
    lp.click("dialog.loot-dialog .loot-tools button:has-text('Lock reserves')")
    lp.wait_for_selector("dialog.loot-dialog .loot-tools button:has-text('Unlock')", timeout=10000)
    assert api("GET", f"/api/events/{SRID}", None, TOKEN)["reserves_locked"]
    # Taking Erin's Choker away (she reserved two).
    lp.click(f"dialog.loot-dialog .sr-trow[data-user='{ERIN}'] .sr-chip:has-text('Choker') .sr-x")
    lp.click("dialog[open]:not(.loot-dialog) button.primary:has-text('Remove')")
    lp.wait_for_function(f"!document.querySelector(\"dialog.loot-dialog .sr-trow[data-user='{ERIN}']\")?.textContent.includes('Choker')", timeout=10000)
    lp.click("dialog.loot-dialog .loot-tools button:has-text('Unlock')")
    lp.wait_for_selector("dialog.loot-dialog .loot-tools button:has-text('Lock reserves')", timeout=10000)
    # Exports: Gargul (default), softres.it CSV and the plain list.
    lp.click("dialog.loot-dialog .loot-tools button[data-act='export']")
    lp.wait_for_selector("dialog.loot-dialog .exp-panel[data-kind='gargul'] .exp-kinds .seg-btn.on:has-text('Gargul')")
    lp.wait_for_selector("dialog.loot-dialog .exp-panel button[data-act='copy-gargul']", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .exp-help:has-text('/gl softreserves')")
    lp.wait_for_selector("dialog.loot-dialog .exp-names select")
    gtext = lp.input_value("dialog.loot-dialog .exp-panel textarea.exp-out")
    gdata = json.loads(zlib.decompress(base64.b64decode(gtext)).decode("ascii"))
    assert gdata["metadata"]["instances"] == ["moltencore"], gdata["metadata"]
    assert gdata["metadata"]["url"].endswith(f"&event={SRID}"), gdata["metadata"]["url"]
    got = {r["name"]: [i["id"] for i in r["items"]] for r in gdata["softreserves"]}
    assert got == {"Carol": [16922], "Dave": [17076], "Erin": [17076]}, got
    assert [r["name"] for r in gdata["softreserves"]] == ["Carol", "Dave", "Erin"]
    lp.context.grant_permissions(["clipboard-read", "clipboard-write"], origin=BASE)
    lp.click("dialog.loot-dialog .exp-panel button[data-act='copy-gargul']")
    lp.wait_for_selector(".toast:has-text('Gargul import copied')", timeout=5000)
    assert lp.evaluate("navigator.clipboard.readText()") == gtext
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/330-desktop-sr-export-gargul.png")
    lp.click("dialog.loot-dialog .exp-kinds .seg-btn[data-kind='csv']")
    lp.wait_for_selector("dialog.loot-dialog .exp-panel[data-kind='csv'] button[data-act='download-csv']")
    csv = lp.input_value("dialog.loot-dialog .exp-panel textarea.exp-out")
    assert csv.startswith("Item Name,Item ID,From,Raider Name,Raider Class,Raider Spec,Raider Note,Extra Reserves,Date\nLeggings of Transcendence,16922,"), csv
    assert len(csv.strip().split("\n")) == 4, csv
    with lp.expect_download() as dl:
        lp.click("dialog.loot-dialog .exp-panel button[data-act='download-csv']")
    assert dl.value.suggested_filename.endswith("-softres.csv"), dl.value.suggested_filename
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/331-desktop-sr-export-csv.png")
    lp.click("dialog.loot-dialog .exp-kinds .seg-btn[data-kind='text']")
    lp.wait_for_selector("dialog.loot-dialog .exp-panel[data-kind='text'] button[data-act='copy-text']")
    text = lp.input_value("dialog.loot-dialog .exp-panel textarea.exp-out")
    assert "Bonereaver's Edge — Dave, Erin" in text, text
    lp.screenshot(path=f"{SHOTS}/332-desktop-sr-export-text.png")
    lp.click("dialog.loot-dialog .loot-tools button[data-act='export']")
    lp.wait_for_selector("dialog.loot-dialog .exp-panel", state="detached")
    # Picking: search the list, reserve, hover for the tooltip.
    lp.click("dialog.loot-dialog .loot-tab:has-text('Reserve')")
    lp.fill("dialog.loot-dialog .loot-search", "bone")
    lp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17076'] .sr-count:has-text('2 SR')")
    lp.click("dialog.loot-dialog .loot-row[data-item='17076'] .sr-pick")
    lp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17076'].mine", timeout=10000)
    mine = [r["item_id"] for r in api("GET", f"/api/events/{SRID}", None, TOKEN)["reserves"] if r["user_id"] == CAROL]
    assert sorted(mine) == [16922, 17076], mine
    lp.fill("dialog.loot-dialog .loot-search", "")
    lp.hover("dialog.loot-dialog .loot-mine .item-name:has-text('Leggings')")
    lp.wait_for_selector("dialog.loot-dialog .item-tip:has-text('Classes: Priest')", timeout=10000)
    lp.wait_for_timeout(150)
    lp.screenshot(path=f"{SHOTS}/155-desktop-sr-picker-tooltip.png")
    lp.mouse.move(5, 5)
    lp.fill("dialog.loot-dialog .loot-search", "thunderfury")
    lp.press("dialog.loot-dialog .loot-search", "Enter")
    lp.wait_for_selector("dialog.loot-dialog .loot-group.found .item-name:has-text('Thunderfury')", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .loot-group.found button:has-text('Add to the Molten Core list')")
    lp.screenshot(path=f"{SHOTS}/156-desktop-sr-lookup.png")
    lp.click("dialog.loot-dialog .loot-group.found button:has-text('Add to the Molten Core list')")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Added by your raid leaders'] .loot-row[data-item='19019']", timeout=10000)
    # The full list from Wowhead (ci/wow_mock.py's Molten Core page), boss by boss.
    lp.fill("dialog.loot-dialog .loot-search", "")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Ragnaros'][open] .loot-row[data-item='17076']", timeout=10000)
    assert lp.query_selector("dialog.loot-dialog .loot-boss[data-boss='Lucifron']:not([open])"), "bosses without reserves start closed"
    bosses = lp.eval_on_selector_all("dialog.loot-dialog .loot-boss", "els => els.map(e => e.dataset.boss)")
    assert bosses[0] == "Lucifron" and bosses.index("Ragnaros") > bosses.index("Golemagg the Incinerator"), bosses
    assert "Smouldering Ash" not in lp.inner_text("dialog.loot-dialog .loot-list"), "grey items are hidden by default"
    href = lp.get_attribute("dialog.loot-dialog .loot-row[data-item='17076'] a.item-name", "href")
    assert href.endswith("/classic/item=17076"), href
    assert lp.get_attribute("dialog.loot-dialog .loot-row[data-item='17076'] a.item-name", "target") == "_blank"
    lp.wait_for_function("[...document.querySelectorAll(\"dialog.loot-dialog .loot-boss[open] img.item-icon\")].some(i => i.complete && i.naturalWidth > 0)", timeout=10000)
    colour = lp.eval_on_selector("dialog.loot-dialog .loot-row[data-item='17076'] .item-label", "e => getComputedStyle(e.closest('a')).color")
    assert colour == "rgb(163, 53, 238)", colour
    lp.click("dialog.loot-dialog .loot-chip.plain:has-text('Expand all')")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Lucifron'][open]")
    lp.click("dialog.loot-dialog .loot-chip.plain:has-text('Collapse all')")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Ragnaros']:not([open])")
    lp.click("dialog.loot-dialog .loot-boss[data-boss='Ragnaros'] > summary")
    lp.click("dialog.loot-dialog .loot-boss[data-boss='Golemagg the Incinerator'] > summary")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{SHOTS}/2600-desktop-sr-picker-bosses.png")
    # Rich tooltip on hover: two columns and Wowhead's colours.
    lp.hover("dialog.loot-dialog .loot-row[data-item='17076'] .item-label")
    lp.wait_for_selector("dialog.loot-dialog .item-tip .tip-line.q2:has-text('Chance on hit')", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .item-tip .tip-right:has-text('Sword')")
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/2601-desktop-item-tooltip.png")
    lp.mouse.move(5, 5)
    # Filters: rings only; what suits Carol's Warrior; grey items on request.
    lp.click("dialog.loot-dialog .loot-chip[data-cat='ring']")
    lp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17110']")
    cats = set(lp.eval_on_selector_all("dialog.loot-dialog .loot-list .loot-row", "els => els.map(e => e.dataset.cat)"))
    assert cats == {"ring"}, cats
    lp.screenshot(path=f"{SHOTS}/2602-desktop-sr-filter-rings.png")
    lp.click("dialog.loot-dialog .loot-chip[data-cat='all']")
    lp.click("dialog.loot-dialog .loot-chip[data-filter='suits']")
    lp.wait_for_selector("dialog.loot-dialog .loot-chip[data-filter='suits'].on")
    assert lp.query_selector("dialog.loot-dialog .loot-list .loot-row[data-item='17076']"), "a two-hand sword suits a Warrior"
    assert not lp.query_selector("dialog.loot-dialog .loot-list .loot-row[data-item='16805']"), "cloth gloves don't"
    lp.screenshot(path=f"{SHOTS}/2603-desktop-sr-suits-warrior.png")
    lp.click("dialog.loot-dialog .loot-chip[data-filter='suits']")
    lp.click("dialog.loot-dialog .loot-chip[data-filter='junk']")
    lp.click("dialog.loot-dialog .loot-chip.plain:has-text('Expand all')")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Trash & other drops'] .loot-row[data-item='99901']")
    lp.click("dialog.loot-dialog .loot-chip[data-filter='junk']")
    lp.click("dialog.loot-dialog .loot-chip.plain:has-text('Collapse all')")
    # Raid leaders: where the list comes from, and importing more by pasting a Wowhead link.
    lp.click("dialog.loot-dialog .loot-tab[data-tab='list']")
    lp.wait_for_selector("dialog.loot-dialog .loot-source[data-source='zone-2717']:has-text('Molten Core')", timeout=10000)
    lp.fill("dialog.loot-dialog .loot-import input", "https://www.wowhead.com/classic/npc=12397/lord-kazzak")
    lp.click("dialog.loot-dialog .loot-import button")
    lp.wait_for_selector("dialog.loot-dialog .loot-source[data-source='npc-12397']:has-text('Lord Kazzak')", timeout=20000)
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/2604-desktop-loot-list-sources.png")
    lp.click("dialog.loot-dialog .loot-list-head button:has-text('Refresh from Wowhead')")
    lp.wait_for_selector("dialog.loot-dialog .loot-list-head button:has-text('Refresh from Wowhead'):not([disabled])", timeout=20000)
    # Or straight in the picker's search box: a chest's link.
    lp.click("dialog.loot-dialog .loot-tab[data-tab='reserve']")
    lp.fill("dialog.loot-dialog .loot-search", "https://www.wowhead.com/classic/object=181366/four-horsemen-chest")
    lp.wait_for_selector("dialog.loot-dialog .loot-import-btn")
    lp.screenshot(path=f"{SHOTS}/2605-desktop-sr-paste-list-link.png")
    lp.click("dialog.loot-dialog .loot-import-btn")
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Four Horsemen Chest']", timeout=20000)
    lp.wait_for_selector("dialog.loot-dialog .loot-boss[data-boss='Lord Kazzak']")
    lp.click("dialog.loot-dialog .raid-close")
    # Loot won on last night's raid: log Thunderfury for Erin.
    scroll_to_card(lp, "Onyxia last night")
    lp.click(f".event-card[data-event='{PRID}'] .ev-link.loot")
    lp.wait_for_selector("dialog.loot-dialog .won-row", timeout=10000)
    lp.fill("dialog.loot-dialog .loot-give input[aria-label='Item']", "Thunderfury")
    lp.select_option("dialog.loot-dialog .loot-give select", ERIN)
    lp.click("dialog.loot-dialog .loot-give button:has-text('Log it')")
    lp.wait_for_selector("dialog.loot-dialog .won-row .item-name:has-text('Thunderfury')", timeout=10000)
    lp.screenshot(path=f"{SHOTS}/157-desktop-loot-won.png")
    lp.click("dialog.loot-dialog .loot-won .link")
    lp.wait_for_selector("dialog.loot-log-dialog .loot-log-table tbody tr >> nth=2", timeout=10000)
    lp.screenshot(path=f"{SHOTS}/158-desktop-loot-log.png")
    lp.select_option("dialog.loot-log-dialog select[aria-label='Player']", DAVE)
    lp.wait_for_function("document.querySelectorAll('dialog.loot-log-dialog tbody tr').length === 1", timeout=10000)
    lp.keyboard.press("Escape")
    lp.wait_for_timeout(300)
    for d in lp.query_selector_all("dialog[open]"):
        lp.keyboard.press("Escape")
    # Dave's profile card lists what he won.
    lp.evaluate(f"openProfile({json.dumps(DAVE)}, document.querySelector('#messages'))")
    lp.wait_for_selector(".profile-pop .pc-loot .item-name:has-text('Sapphiron Drape')", timeout=10000)
    lp.screenshot(path=f"{SHOTS}/159-desktop-profile-loot.png")
    lp.keyboard.press("Escape")
    # Plan a raid: the loot list is guessed from the name, soft reserves off by default.
    lp.evaluate("openRaidDialog()")
    lp.wait_for_selector(".raid-dialog .loot-fields select[aria-label='Loot list'] option[value='naxx']", state="attached", timeout=10000)
    lp.fill(".raid-dialog input[placeholder^='Raid night']", "Naxx clear")
    lp.wait_for_function("document.querySelector(\".raid-dialog select[aria-label='Loot list']\").value === 'naxx'", timeout=5000)
    lp.select_option(".raid-dialog select[aria-label='Soft reserves']", "1")
    lp.wait_for_selector(".raid-dialog select[aria-label='Reserves lock']:visible")
    lp.evaluate("document.querySelector('.raid-dialog .loot-fields').scrollIntoView({block: 'center'})")
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{SHOTS}/160-desktop-plan-raid-loot.png")
    lp.click(".raid-dialog button:has-text('Post raid')")
    lp.wait_for_selector(".event-card:has(.ev-title:has-text('Naxx clear')) .ev-link.sr[title*='Naxxramas']", timeout=10000)
    lp.close()

    # Dave on a phone: the composition and picking his reserves.
    for label, kw, n in (("phone", {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}, 161),
                         ("android", {"viewport": {"width": 393, "height": 852}, "is_mobile": True, "has_touch": True}, 165)):
        mp = loot_page(dave["token"] if label == "phone" else TOKEN, f"loot-{label}", **kw)
        card = sr_card(mp)
        mp.click(f"{card} .ev-comp > summary")
        mp.wait_for_selector(f"{card} .comp-buff[data-buff='fortitude'].yes", timeout=10000)
        mp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{card} .ev-comp")
        mp.wait_for_timeout(250)
        mp.screenshot(path=f"{SHOTS}/{n}-{label}-composition.png")
        if label == "phone":
            mp.click(f"{card} .ev-link.sr")
            mp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17076']", timeout=10000)
            mp.wait_for_function("[...document.querySelectorAll(\"dialog.loot-dialog .loot-boss[open] img.item-icon\")].some(i => i.complete && i.naturalWidth > 0)", timeout=10000)
            mp.wait_for_timeout(250)
            mp.screenshot(path=f"{SHOTS}/{n + 1}-{label}-sr-picker.png")
            # A tap shows the tooltip (with a link to Wowhead) instead of leaving the app.
            mp.tap("dialog.loot-dialog .loot-row[data-item='17076'] .item-label")
            mp.wait_for_selector("dialog.loot-dialog .item-tip.touch .item-tip-open", timeout=10000)
            mp.wait_for_selector("dialog.loot-dialog .item-tip .tip-line.q2")
            mp.wait_for_timeout(200)
            mp.screenshot(path=f"{SHOTS}/26{n - 150}-{label}-item-tooltip.png")
            mp.tap("dialog.loot-dialog .loot-status")
            mp.wait_for_selector("dialog.loot-dialog .item-tip", state="detached")
            # Bonereaver's Edge is his already; one more (two each).
            mp.click("dialog.loot-dialog .loot-row[data-item='17204'] .sr-pick")
            mp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17204'].mine", timeout=10000)
            mp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='18814'] .sr-pick[disabled]")
            mp.click("dialog.loot-dialog .loot-tab:has-text(\"Everyone's\")")
            mp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='17076']")
            assert not mp.query_selector("dialog.loot-dialog .loot-tools"), "members don't get the leader's tools"
            mp.screenshot(path=f"{SHOTS}/{n + 2}-{label}-sr-everyone.png")
            mp.click("dialog.loot-dialog .raid-close")
            mp.evaluate("openLootLog()")
        else:
            mp.click(f"{card} .ev-link.sr")
            mp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='17076']", timeout=10000)
            mp.wait_for_timeout(250)
            mp.screenshot(path=f"{SHOTS}/{n + 1}-{label}-sr-table.png")
            mp.click("dialog.loot-dialog .loot-tools button[data-act='export']")
            mp.wait_for_selector("dialog.loot-dialog .exp-panel button[data-act='copy-gargul']", timeout=10000)
            mp.evaluate("document.querySelector('dialog.loot-dialog .exp-panel')?.scrollIntoView({block: 'start'})")
            mp.wait_for_timeout(250)
            mp.screenshot(path=f"{SHOTS}/333-{label}-sr-export-gargul.png")
            mp.click("dialog.loot-dialog .loot-tools button[data-act='export']")
            mp.click("dialog.loot-dialog .loot-tab:has-text('Reserve')")
            mp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='17076']")
            mp.wait_for_timeout(300)
            mp.screenshot(path=f"{SHOTS}/{n + 2}-{label}-sr-picker.png")
            mp.click("dialog.loot-dialog .loot-tab[data-tab='list']")
            mp.wait_for_selector("dialog.loot-dialog .loot-source[data-source='npc-12397']", timeout=10000)
            mp.wait_for_timeout(200)
            mp.screenshot(path=f"{SHOTS}/26{n - 150}-{label}-loot-list.png")
            mp.click("dialog.loot-dialog .raid-close")
            mp.evaluate("openLootLog()")
        mp.wait_for_selector("dialog.loot-log-dialog .loot-log-table tbody tr", timeout=10000)
        mp.wait_for_timeout(200)
        mp.screenshot(path=f"{SHOTS}/{n + 3}-{label}-loot-log.png")
        mp.close()
    dave_res = [r["item_id"] for r in api("GET", f"/api/events/{SRID}", None, TOKEN)["reserves"] if r["user_id"] == DAVE]
    assert sorted(dave_res) == [17076, 17204], dave_res

    # ---- The orange update button (updates.js) ----
    # Windows app: a mocked app bridge (http://outhouse.localhost) that finds
    # an update, then downloads it with progress after "Update now".
    def fake_bridge(pg, origin=BASE):
        st = {"status": {"enabled": True, "current": "0.2.40", "available": False}, "calls": []}

        def handle(route):
            url = route.request.url
            st["calls"].append(url)
            headers = {"Access-Control-Allow-Origin": origin}
            if "/update-status" in url:
                s = st["status"]
                if s.get("downloading"):
                    s["progress"] = round(min(0.9, (s.get("progress") or 0) + 0.15), 2)
                route.fulfill(status=200, headers=headers, content_type="application/json", body=json.dumps(s))
            elif "/update-install" in url:
                if not st["status"].get("available"):
                    route.fulfill(status=409, headers=headers, body="there's no update to install")
                    return
                st["status"].update({"downloading": True, "progress": 0.1})
                route.fulfill(status=200, headers=headers, body="ok")
            else:
                route.fulfill(status=200, headers=headers, body="ok")
        pg.route("http://outhouse.localhost/**", handle)
        return st

    def app_page(label, vp, platform, token=TOKEN, mobile=False):
        pg = browser.new_page(viewport=vp, is_mobile=mobile, has_touch=mobile, permissions=["microphone"])
        watch(pg, label)
        flags = "" if platform == "browser" else f"window.__OUTHOUSE_DESKTOP__ = true; window.__OUTHOUSE_PLATFORM__ = '{platform}'; window.__OUTHOUSE_APP_VERSION__ = '0.2.40';"
        pg.add_init_script(flags + f"localStorage.removeItem('outhouse_update_later'); localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(chan['id'])});")
        return pg

    NOTES = "Voice reconnects faster after a drop.\nThe raid planner remembers your spec."
    for label, vp, mobile, base in (("desktop", {"width": 1280, "height": 800}, False, 3401), ("phone", {"width": 390, "height": 844}, True, 3411)):
        up = app_page(f"update-win-{label}", vp, "desktop", mobile=mobile)
        bridge = fake_bridge(up)
        up.goto(BASE + "/")
        up.wait_for_selector("#messages .msg", timeout=15000)
        up.wait_for_function("updates.state.app !== null", timeout=10000)
        assert up.is_hidden("#update-btn"), "update button shows with no update"
        up.screenshot(path=f"{SHOTS}/{base}-{label}-update-none.png")
        # The app's hourly check finds 0.2.41 and nudges the page.
        bridge["status"].update({"available": True, "version": "0.2.41", "notes": NOTES, "date": "2026-11-09 08:00:00.0 +00:00:00"})
        up.evaluate("window.dispatchEvent(new Event('outhouse-update'))")
        up.wait_for_selector("#update-btn:not([hidden])", timeout=5000)
        title = up.get_attribute("#update-btn", "title")
        assert title == "Update available — Outhouse 0.2.41. Click to restart and update", title
        assert up.evaluate("getComputedStyle(document.getElementById('update-btn')).color") == "rgb(255, 107, 53)", "not ember orange"
        up.wait_for_timeout(1200)  # mid-glow
        up.screenshot(path=f"{SHOTS}/{base + 1}-{label}-update-available.png")
        # Later hides it for six hours; a newer version brings it back.
        up.click("#update-btn")
        up.wait_for_selector(".upd-pop #update-later")
        up.click(".upd-pop #update-later")
        up.wait_for_selector("#update-btn", state="hidden", timeout=3000)
        assert up.evaluate("JSON.parse(localStorage.getItem('outhouse_update_later')).key") == "app:0.2.41"
        up.evaluate("updates.check()")
        up.wait_for_timeout(300)
        assert up.is_hidden("#update-btn"), "Later didn't stick"
        bridge["status"]["version"] = "0.2.42"
        up.evaluate("updates.check()")
        up.wait_for_selector("#update-btn:not([hidden])", timeout=5000)
        # The popover: version, notes, Later / Update now.
        up.click("#update-btn")
        up.wait_for_selector(".upd-pop .upd-notes:has-text('remembers your spec')")
        assert "Outhouse 0.2.42" in up.inner_text(".upd-pop .upd-sub")
        assert up.inner_text(".upd-pop #update-now") == "Update now"
        assert up.query_selector(".upd-pop .upd-warn") is None
        up.screenshot(path=f"{SHOTS}/{base + 2}-{label}-update-popover.png")
        # Clicking the button again closes it.
        up.click("#update-btn")
        up.wait_for_selector(".upd-pop", state="detached", timeout=3000)
        if label == "desktop":
            # In voice: it warns first, and leaves voice before updating.
            up.click("#voice-channels .channel.voice:has-text('Raid comms')")
            up.wait_for_function("state.voice.room && state.voice.room.state === 'connected'", timeout=20000)
            up.click("#update-btn")
            up.wait_for_selector(".upd-pop .upd-warn:has-text('This will disconnect you from voice')")
            assert up.inner_text(".upd-pop #update-now") == "Leave voice and update"
            up.screenshot(path=f"{SHOTS}/{base + 3}-{label}-update-voice-warning.png")
        else:
            up.click("#update-btn")
            up.wait_for_selector(".upd-pop #update-now")
        up.click(".upd-pop #update-now")
        up.wait_for_selector(".upd-pop .upd-progress", timeout=5000)
        up.wait_for_function("updates.state.app?.progress >= 0.4", timeout=10000)
        assert up.evaluate("!state.voice.room && !state.voice.channelId"), "still in voice after Update now"
        assert any("/update-install" in c for c in bridge["calls"]), "update-install wasn't called"
        assert "Downloading" in up.inner_text(".upd-pop .upd-status")
        up.screenshot(path=f"{SHOTS}/{base + 4}-{label}-update-downloading.png")
        bridge["status"].update({"downloading": False, "ready": True, "progress": 1})
        up.wait_for_selector(".upd-pop .upd-status:has-text('Restarting…')", timeout=5000)
        up.screenshot(path=f"{SHOTS}/{base + 5}-{label}-update-restarting.png")
        up.close()

    # An older Windows app without the update endpoints: nothing shows, and
    # the desktop connection isn't marked broken.
    old = app_page("update-old-app", {"width": 1280, "height": 800}, "desktop")
    old.route("http://outhouse.localhost/**", lambda route: route.fulfill(status=404 if "update" in route.request.url else 200,
              headers={"Access-Control-Allow-Origin": BASE}, body="unknown" if "update" in route.request.url else "ok"))
    old.goto(BASE + "/")
    old.wait_for_selector("#messages .msg", timeout=15000)
    old.wait_for_function("updates.state.bridgeGone === true", timeout=10000)
    assert old.is_hidden("#update-btn")
    assert old.evaluate("desktopStatus.ok"), "an older app's missing update endpoint marked the bridge broken"
    old.close()

    # Android app: /updates/android.json is newer, the button offers the APK.
    for label, vp, base in (("android", {"width": 393, "height": 852}, 3421),):
        ad = app_page(f"update-{label}", vp, "android", mobile=True)
        abridge = fake_bridge(ad)
        ad.route("**/updates/android.json", lambda route: route.fulfill(json={"version": "0.2.41", "url": "/updates/Outhouse-Android.apk"}))
        ad.goto(BASE + "/")
        ad.wait_for_selector("#messages .msg", timeout=15000)
        ad.wait_for_selector("#update-btn:not([hidden])", timeout=10000)
        assert ad.query_selector(".update-bar") is None, "the old banner is still there"
        assert ad.get_attribute("#update-btn", "title") == "Update available — Outhouse 0.2.41. Click to download it"
        ad.wait_for_timeout(1200)
        ad.screenshot(path=f"{SHOTS}/{base}-{label}-update-available.png")
        ad.click("#update-btn")
        ad.wait_for_selector(".upd-pop #update-now:has-text('Download')")
        ad.screenshot(path=f"{SHOTS}/{base + 1}-{label}-update-popover.png")
        ad.click(".upd-pop #update-now")
        ad.wait_for_timeout(500)
        opened = [c for c in abridge["calls"] if "/open?" in c]
        assert opened and "Outhouse-Android.apk" in urllib.parse.unquote(opened[-1]), abridge["calls"]
        ad.close()
    # Same version as the app: no button.
    ad = app_page("update-android-current", {"width": 393, "height": 852}, "android", mobile=True)
    ad.route("**/updates/android.json", lambda route: route.fulfill(json={"version": "0.2.40", "url": "/updates/Outhouse-Android.apk"}))
    ad.goto(BASE + "/")
    ad.wait_for_selector("#messages .msg", timeout=15000)
    ad.wait_for_function("updates.state.lastApk > 0", timeout=10000)
    ad.wait_for_timeout(500)
    assert ad.is_hidden("#update-btn"), "update button for the same Android version"
    ad.close()

    # Plain browser: the server got a newer web client (web_build changed).
    for label, vp, mobile, base in (("browser", {"width": 1280, "height": 800}, False, 3431), ("browser-phone", {"width": 390, "height": 844}, True, 3434)):
        wb = app_page(f"update-{label}", vp, "browser", mobile=mobile)
        swap = {"on": False}

        def health_route(swap):
            # (One argument: Playwright passes the request too to two-argument handlers.)
            def handle(route):
                if not swap["on"]:
                    route.continue_()
                    return
                res = route.fetch()
                body = res.json()
                body["web_build"] = "feedfacefeedface"
                route.fulfill(response=res, json=body)
            return handle
        wb.route("**/api/health", health_route(swap))
        wb.goto(BASE + "/")
        wb.wait_for_selector("#messages .msg", timeout=15000)
        wb.wait_for_function("!!updates.state.webBuild", timeout=10000)
        assert wb.is_hidden("#update-btn")
        swap["on"] = True
        wb.evaluate("checkForUpdates()")
        wb.wait_for_selector("#update-btn:not([hidden])", timeout=5000)
        assert "Click to refresh" in wb.get_attribute("#update-btn", "title")
        wb.wait_for_timeout(1200)
        wb.screenshot(path=f"{SHOTS}/{base}-{label}-refresh-available.png")
        wb.click("#update-btn")
        wb.wait_for_selector(".upd-pop #update-now:has-text('Refresh to update')")
        wb.screenshot(path=f"{SHOTS}/{base + 1}-{label}-refresh-popover.png")
        with wb.expect_navigation(timeout=10000):
            wb.click(".upd-pop #update-now")
        wb.wait_for_selector("#messages .msg", timeout=15000)
        wb.wait_for_function("updates.state.webBuild === 'feedfacefeedface'", timeout=10000)
        assert wb.is_hidden("#update-btn"), "still offering a refresh after refreshing"
        wb.close()

    # Home (DMs and friends): the button is there too.
    hm = app_page("update-home", {"width": 1280, "height": 800}, "desktop")
    hb = fake_bridge(hm)
    hb["status"].update({"available": True, "version": "0.2.41"})
    hm.goto(BASE + "/")
    hm.wait_for_selector("#messages .msg", timeout=15000)
    hm.click("#home-btn")
    hm.wait_for_selector("#update-btn:not([hidden])", timeout=10000)
    hm.wait_for_timeout(800)
    box = hm.query_selector("#update-btn").bounding_box()
    head = hm.query_selector(".chat-head").bounding_box()
    # Right-hand end of the header (only the connection dot and phone toggles come after it).
    assert box and box["y"] < 56 and head["x"] + head["width"] - (box["x"] + box["width"]) < 80, f"update button not at the top right on Home: {box} in {head}"
    hm.screenshot(path=f"{SHOTS}/3441-home-update-available.png")
    hm.close()

    # ---- Safety: reports, the owner's console, suspension, teen protections (safety.js) ----
    def reg_from(ip, name, display, dob="1990-01-01"):
        """A sign-up from its own address (sign-ups are limited per address)."""
        req = urllib.request.Request(BASE + "/api/auth/register", method="POST", data=json.dumps(
            {"username": name, "password": "password123", "display_name": display, "birth_date": dob}).encode())
        req.add_header("Content-Type", "application/json")
        req.add_header("X-Forwarded-For", ip)
        with urllib.request.urlopen(req) as res:
            return json.loads(res.read().decode())

    teen_dob = time.strftime("%Y-%m-%d", time.gmtime(time.time() - 15 * 365.25 * 86400))
    tess = reg_from("198.18.9.1", f"tess{suffix}", "Tess", teen_dob)
    gary = reg_from("198.18.9.2", f"gary{suffix}", "Gary")
    hal = reg_from("198.18.9.3", f"hal{suffix}", "Hal")
    rita = reg_from("198.18.9.4", f"rita{suffix}", "Rita")
    for u in (tess, gary, hal, rita):
        api("POST", f"/api/invites/{code}", None, u["token"])
    rep_chan = api("POST", f"/api/guilds/{guild['id']}/channels", {"name": "report-test"}, TOKEN)
    api("POST", f"/api/channels/{rep_chan['id']}/messages", {"content": "Evening all, raid at 8?"}, hal["token"])
    gary_msg = api("POST", f"/api/channels/{rep_chan['id']}/messages", {"content": "hey kid, want free gold? add me and DM me your password"}, gary["token"])
    cs_msg = upload(f"/api/channels/{rep_chan['id']}/messages/upload", {"content": "look what I found"},
                    {"files": ("photo.png", make_png(64), "image/png")}, gary["token"])
    cs_url = cs_msg["attachments"][0]["url"]
    api("POST", f"/api/channels/{rep_chan['id']}/messages", {"content": "That's not ok, reporting it."}, hal["token"])
    api("POST", "/api/reports", {"target_kind": "message", "target_id": cs_msg["id"], "category": "child_safety",
                                 "details": "Picture of a child, sent to the channel"}, hal["token"])
    api("POST", "/api/reports", {"target_kind": "user", "target_id": gary["user"]["id"], "category": "spam"}, hal["token"])

    def safety_page(token, vp=None, mobile=False, channel=None):
        pg = browser.new_page(viewport=vp or {"width": 1280, "height": 800}, is_mobile=mobile, has_touch=mobile)
        watch(pg, f"safety-{'phone' if mobile else 'desktop'}")
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(channel or rep_chan['id'])});")
        pg.goto(BASE + "/")
        pg.wait_for_selector("#messages .msg", timeout=15000)
        return pg

    # Report dialog (desktop): right-click Gary's message → Report message.
    rp = safety_page(rita["token"])
    rp.wait_for_selector(f"#msg-{gary_msg['id']}")
    rp.click(f"#msg-{gary_msg['id']} .msg-text", button="right")
    rp.click(".menu button:has-text('Report message')")
    rp.wait_for_selector("dialog.report-dialog[open]")
    assert rp.inner_text("dialog.report-dialog .rp-option >> nth=0").startswith("Child safety"), "child safety isn't first"
    rp.click("dialog.report-dialog .rp-option:has-text('Harassment or bullying')")
    rp.fill("dialog.report-dialog textarea", "Asking a kid for their password in #report-test.")
    rp.check("dialog.report-dialog .rp-block")
    rp.screenshot(path=f"{SHOTS}/3501-report-dialog-desktop.png")
    rp.click("dialog.report-dialog button[type=submit]")
    rp.wait_for_selector(".toast:has-text('Your report was sent')", timeout=10000)
    rp.wait_for_selector(f"#msg-{gary_msg['id']}.blocked-hidden", state="attached", timeout=10000)
    rp.close()
    reps = api("GET", "/api/safety/reports?status=open", None, TOKEN)["reports"]
    assert any(r["target_id"] == gary_msg["id"] and r["category"] == "harassment" for r in reps), reps[:3]
    assert reps[0]["child_safety"], "child-safety report isn't first"
    # The quarantined picture is gone for everyone, and its message is hidden.
    try:
        urllib.request.urlopen(BASE + cs_url)
        errors.append("quarantined file was served")
    except urllib.error.HTTPError as e:
        assert e.code == 451, e.code

    # Report dialog (phone): the profile card's ⋯ → Report…, child safety picked.
    pp = safety_page(rita["token"], {"width": 390, "height": 844}, mobile=True)
    pp.evaluate(f"openProfile({json.dumps(hal['user']['id'])}, document.querySelector('#messages .msg .msg-author'))")
    pp.wait_for_selector(".profile-card .pc-more")
    pp.click(".profile-card .pc-more")
    pp.click(".menu button:has-text('Report…')")
    pp.wait_for_selector("dialog.report-dialog[open]")
    pp.click("dialog.report-dialog .rp-option:has-text('Child safety')")
    pp.wait_for_selector("dialog.report-dialog .rp-urgent:not([hidden])")
    pp.screenshot(path=f"{SHOTS}/3502-report-dialog-phone.png")
    pp.click("dialog.report-dialog button:has-text('Cancel')")
    pp.wait_for_selector("dialog.report-dialog", state="detached")
    pp.close()

    # The owner's inbox: shield on the rail with a count, child safety first.
    op = safety_page(TOKEN)
    seen_files = []
    op.on("request", lambda r: "/files/" in r.url and seen_files.append(r.url))
    op.wait_for_selector("#safety-btn .mention-badge.urgent", timeout=10000)
    op.click("#safety-btn")
    op.wait_for_selector("dialog.safety-console .sc-row.child", timeout=10000)
    op.wait_for_timeout(300)
    op.screenshot(path=f"{SHOTS}/3503-owner-inbox.png")
    # Child-safety detail: a locked placeholder and the authority links, never the picture.
    op.click("dialog.safety-console .sc-row.child >> nth=0")
    op.wait_for_selector("dialog.safety-console .sc-child-warning", timeout=10000)
    op.wait_for_selector("dialog.safety-console .sc-locked")
    assert op.query_selector("dialog.safety-console .sc-files img, dialog.safety-console .sc-files video") is None, "child-safety media shown"
    assert op.query_selector("dialog.safety-console .sc-file button:has-text('Show')") is None, "child-safety file has a Show button"
    warn = op.inner_text("dialog.safety-console .sc-child-warning")
    for needle in ("accce.gov.au/report", "esafety.gov.au/report", "000", "report.cybertip.org", "Do not view, download"):
        assert needle in warn, needle
    assert "sha-256" in op.inner_text("dialog.safety-console .sc-files").lower()
    op.screenshot(path=f"{SHOTS}/3504-report-detail-child-safety.png")
    assert not any("photo" in u or cs_url in u for u in seen_files), seen_files
    # Suspension: from Gary's spam report.
    op.click("dialog.safety-console .sc-back button")
    op.click("dialog.safety-console .sc-row:has-text('Spam or scams'):has-text('Gary')")
    op.wait_for_selector("dialog.safety-console .sc-suspend", timeout=10000)
    op.click("dialog.safety-console .sc-suspend")
    op.wait_for_selector("dialog.sc-form[open]")
    op.select_option("dialog.sc-form select", "7")
    op.fill("dialog.sc-form input[type=text]", "Asking kids for passwords")
    op.check("dialog.sc-form input[type=checkbox]")
    op.screenshot(path=f"{SHOTS}/3505-suspend-dialog.png")
    op.click("dialog.sc-form button[type=submit]")
    op.wait_for_selector("dialog.safety-console .rp-chip:has-text('Suspended until')", timeout=10000)
    op.wait_for_timeout(300)
    op.screenshot(path=f"{SHOTS}/3506-suspended.png")
    try:
        api("GET", "/api/users/@me", None, gary["token"])
        errors.append("suspended account still signed in")
    except urllib.error.HTTPError as e:
        assert e.code == 401, e.code
    op.keyboard.press("Escape")
    # Outhouse settings → Reports (site owner only).
    op.evaluate("openSettings('reports')")
    op.wait_for_selector("dialog.settings .settings-main h2:text-is('Reports')", timeout=10000)
    op.wait_for_selector("dialog.settings .settings-main .sc-row")
    op.keyboard.press("Escape")
    op.close()
    # Phone: the console fits.
    opp = safety_page(TOKEN, {"width": 390, "height": 844}, mobile=True)
    opp.evaluate("openSafetyConsole()")
    opp.wait_for_selector("dialog.safety-console .sc-row", timeout=10000)
    opp.wait_for_timeout(300)
    opp.screenshot(path=f"{SHOTS}/3507-owner-inbox-phone.png")
    opp.close()
    # Not the owner: no shield, and the console's API says no.
    assert api("GET", "/api/users/@me/site-owner", None, dave["token"])["site_owner"] is False

    # A teen: adults who aren't their friend see "This person can't receive messages from you".
    tp = safety_page(hal["token"])
    tp.evaluate(f"openProfile({json.dumps(tess['user']['id'])}, document.querySelector('#messages .msg .msg-author'))")
    tp.wait_for_selector(".profile-card .pc-cant:has-text(\"This person can't receive messages from you.\")", timeout=10000)
    assert tp.is_disabled(".profile-card .pc-message")
    tp.wait_for_timeout(200)
    tp.screenshot(path=f"{SHOTS}/3508-teen-cant-message.png")
    tp.close()

    # Sign-up asks for a date of birth; under 13s are turned away.
    sp_ = browser.new_page(viewport={"width": 1280, "height": 800}, extra_http_headers={"X-Forwarded-For": "198.18.9.20"})
    watch(sp_, "signup-age")
    sp_.goto(BASE + "/app?register")
    sp_.wait_for_selector("input[name=birth_date]", timeout=10000)
    sp_.fill("input[name=username]", f"littlekid{suffix}")
    sp_.fill("input[name=password]", "password123")
    sp_.fill("input[name=birth_date]", time.strftime("%Y-%m-%d", time.gmtime(time.time() - 9 * 365.25 * 86400)))
    sp_.click("#auth-submit")
    sp_.wait_for_selector("#auth-error:has-text('13 or older')", timeout=10000)
    sp_.screenshot(path=f"{SHOTS}/3509-signup-under-13.png")
    sp_.close()

    browser.close()

    # Scrollbars (headless Chrome hides them unless asked): a long chat at
    # desktop and phone widths, with the pointer over the messages.
    sb_browser = p.chromium.launch(ignore_default_args=["--hide-scrollbars"])
    for label, vp, shot in (("desktop", {"width": 1280, "height": 800}, 2111), ("phone", {"width": 390, "height": 844}, 2112)):
        sb = sb_browser.new_page(viewport=vp, bypass_csp=True)
        watch(sb, f"scrollbars-{label}")
        sb.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(guild['id'])}); localStorage.setItem('outhouse_channel_{guild['id']}', {json.dumps(general['id'])});")
        sb.goto(BASE + "/")
        sb.wait_for_selector("#messages .msg:has-text('filler chatter')", timeout=15000)
        sb.wait_for_timeout(500)
        box = sb.query_selector("#messages").bounding_box()
        sb.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        sb.wait_for_timeout(200)
        thumb = sb.evaluate("getComputedStyle(document.querySelector('#messages'), '::-webkit-scrollbar').width")
        sb.screenshot(path=f"{SHOTS}/{shot}-{label}-scrollbars.png")
        print(f"scrollbar width ({label}): {thumb}")
        sb.close()
    sb_browser.close()

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("UI test passed.")
