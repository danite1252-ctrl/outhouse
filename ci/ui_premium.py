"""Premium (Golden Throne / Golden Outhouse) in headless Chrome, desktop and
phone: the Golden Throne page locked and active (themes, profile colours,
free slot, My sounds), app themes, the Golden Outhouse page and the vanity
link in Outhouse settings, the invite page with a vanity link and the gold
badge, an animated avatar and the gradient on a profile card, the emoji
picker with other outhouses' emoji (locked and usable), My sounds on the
soundboard, and the owner's Grant dialogs and Premium page. Fails on any
JavaScript error and saves screenshots (99xx-*.png).

Needs the server with DATABASE_URL (for psql) and a site owner: OWNER_USER
(default carolui, made by ci/ui_test.py) signs in with password123, or is
registered when this runs on a new database.
"""

import json
import os
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
DB = os.environ["DATABASE_URL"]
OWNER_USER = os.environ.get("OWNER_USER", "carolui")
os.makedirs(SHOTS, exist_ok=True)
errors = []
suffix = str(int(time.time()))[-5:]
DESKTOP = {"viewport": {"width": 1280, "height": 800}}
PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
ip_n = [0]


def request(method, path, data=None, token=None, content_type="application/json", ok=True):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", content_type)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    ip_n[0] += 1
    req.add_header("X-Forwarded-For", f"198.18.99.{ip_n[0] % 250 + 1}")
    try:
        with urllib.request.urlopen(req, data) as res:
            text = res.read().decode()
            return json.loads(text) if text else None
    except urllib.error.HTTPError as e:
        if not ok:
            return {"status": e.code, **json.loads(e.read().decode() or "{}")}
        sys.exit(f"{method} {path} -> {e.code}: {e.read().decode()}")


def api(method, path, body=None, token=None, ok=True):
    return request(method, path, json.dumps(body).encode() if body is not None else None, token, ok=ok)


def upload(path, token, fields, files):
    """Multipart: fields {name: value}, files {name: (filename, bytes, type)}."""
    boundary = uuid.uuid4().hex
    out = b""
    for k, v in fields.items():
        out += f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n".encode()
    for k, (fname, data, ctype) in files.items():
        out += f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"; filename=\"{fname}\"\r\nContent-Type: {ctype}\r\n\r\n".encode() + data + b"\r\n"
    out += f"--{boundary}--\r\n".encode()
    return request("POST", path, out, token, content_type=f"multipart/form-data; boundary={boundary}")


def psql(sql):
    return subprocess.run(["psql", DB, "-tAc", sql], check=True, capture_output=True, text=True).stdout.strip()


def person(name, display):
    r = api("POST", "/api/auth/register", {
        "username": name, "email": f"{name}@example.test", "password": "password123",
        "birth_date": "1990-01-01", "display_name": display,
    })
    return r["token"], r["user"]["id"]


def owner_token():
    r = api("POST", "/api/auth/login", {"username": OWNER_USER, "password": "password123"}, ok=False)
    if r.get("token"):
        return r["token"]
    return person(OWNER_USER, "Carol")[0]


def gif(colours, size=1):
    """An animated GIF: one solid-colour frame per (r, g, b), `size` px square."""
    pal = b"".join(bytes(c) for c in colours) + b"\x00\x00\x00" * (8 - len(colours))
    g = b"GIF89a" + struct.pack("<HH", size, size) + b"\xf2\x00\x00" + pal
    g += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    n = size * size
    for i in range(len(colours)):
        # Uncompressed LZW (code size 3: clear=8, end=9, 4-bit codes), clear
        # before every 6 pixels so the code width never grows.
        codes = []
        for j in range(n):
            if j % 6 == 0:
                codes.append(8)
            codes.append(i)
        codes.append(9)
        bits = 0
        nbits = 0
        data = bytearray()
        for c in codes:
            bits |= c << nbits
            nbits += 4
            while nbits >= 8:
                data.append(bits & 0xFF)
                bits >>= 8
                nbits -= 8
        if nbits:
            data.append(bits & 0xFF)
        blocks = b"".join(bytes([len(data[k:k + 255])]) + bytes(data[k:k + 255]) for k in range(0, len(data), 255))
        g += b"\x21\xf9\x04\x04\x28\x00\x00\x00"
        g += b"\x2c" + struct.pack("<HHHH", 0, 0, size, size) + b"\x00" + b"\x03" + blocks + b"\x00"
    return g + b"\x3b"


def wav(seconds=1, rate=8000):
    n = int(rate * seconds)
    return b"RIFF" + struct.pack("<I", 36 + n) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate, 1, 8) + b"data" + struct.pack("<I", n) + bytes([128]) * n


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text
            and "Failed to load resource" not in m.text and errors.append(f"[{label}] console: {m.text}"))


def shot(pg, name):
    pg.wait_for_timeout(400)
    pg.screenshot(path=f"{SHOTS}/{name}.png")


OWNER = owner_token()
assert api("GET", "/api/users/@me/site-owner", None, OWNER)["site_owner"], f"{OWNER_USER} isn't the site owner"
GT, GT_ID = person(f"goldie{suffix}", "Goldie")
PL, PL_ID = person(f"plainjane{suffix}", "Plain Jane")
GA = api("POST", "/api/guilds", {"name": f"Throne Room {suffix}"}, GT)
GB = api("POST", "/api/guilds", {"name": f"Long Drop {suffix}"}, PL)
for g, t in ((GA, GT), (GB, PL)):
    api("POST", f"/api/guilds/{g['id']}/channels", {"name": "general"}, t)
    api("POST", f"/api/guilds/{g['id']}/channels", {"name": "Voice", "kind": 2}, t)
inv_a = api("POST", f"/api/guilds/{GA['id']}/invites", {"max_age_secs": 0}, GT)["code"]
inv_b = api("POST", f"/api/guilds/{GB['id']}/invites", {"max_age_secs": 0}, PL)["code"]
api("POST", f"/api/invites/{inv_a}", None, PL)
api("POST", f"/api/invites/{inv_b}", None, GT)
# Emoji in the Long Drop (the other outhouse, for Goldie).
for name, col in (("dunny", (255, 140, 0)), ("plunger", (230, 180, 34)), ("loo_roll", (240, 240, 240))):
    upload(f"/api/guilds/{GB['id']}/emojis", PL, {"name": name}, {"file": (f"{name}.gif", gif([col], 8), "image/gif")})
# An animated icon for the Throne Room and an animated avatar for Goldie.
upload(f"/api/guilds/{GA['id']}/icon", GT, {}, {"file": ("icon.gif", gif([(230, 180, 34), (255, 107, 53), (120, 60, 220)], 8), "image/gif")})
upload("/api/users/@me/avatar", GT, {}, {"file": ("me.gif", gif([(91, 140, 255), (230, 180, 34)], 8), "image/gif")})

with sync_playwright() as p:
    browser = p.chromium.launch(**({"executable_path": os.environ["CHROME"]} if os.environ.get("CHROME") else {}))
    # wait_for_function compiles its expression with new Function(), which
    # the site's CSP refuses (ci/csp_test.py checks the CSP itself).
    _new_page = browser.new_page
    browser.new_page = lambda *a, **kw: _new_page(*a, **{"bypass_csp": True, **kw})

    def signed_in(label, kw, token, guild=None):
        pg = browser.new_page(**kw)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)});")
        pg.goto(BASE + "/app")
        pg.wait_for_selector("#app:not([hidden])", timeout=15000)
        pg.wait_for_function("state.ws && state.ws.readyState === 1 && state.premium", timeout=15000)
        if guild:
            pg.evaluate(f"selectGuild({json.dumps(guild)})")
            pg.wait_for_function(f"state.guildId === {json.dumps(guild)} && state.channelId", timeout=10000)
        return pg

    def user_page(pg, tab):
        pg.evaluate(f"openUserSettings({json.dumps(tab)})")
        pg.wait_for_selector("dialog.settings .gold-hero", timeout=15000)
        pg.wait_for_selector("dialog.settings #gt-sounds .gold-sounds, dialog.settings #gt-sounds p.muted.small:not(:has-text('Loading'))", timeout=10000)

    # ---- not premium: everything locked, High contrast still free ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"locked-{label}", kw, GT, GA["id"])
        user_page(pg, "golden")
        assert "coming soon" in pg.inner_text("dialog.settings .gold-hero")
        assert pg.is_disabled("dialog.settings #gt-slot button.primary")
        shot(pg, f"9901-golden-throne-locked-{label}")
        pg.evaluate("document.querySelector('dialog.settings #gt-themes').scrollIntoView()")
        shot(pg, f"9902-themes-locked-{label}")
        if label == "desktop":
            pg.click("dialog.settings .gold-theme[data-theme=throne]")
            pg.wait_for_selector(".toast:has-text('coming soon')", timeout=5000)
            pg.click("dialog.settings .gold-theme[data-theme=contrast]")
            pg.wait_for_function("document.documentElement.dataset.theme === 'contrast'", timeout=5000)
            shot(pg, "9903-high-contrast-free")
            pg.click("dialog.settings .gold-theme[data-theme=ember]")
            pg.wait_for_function("!document.documentElement.dataset.theme", timeout=5000)
            # The emoji picker: the Long Drop's emoji are locked here.
            pg.keyboard.press("Escape")
            pg.wait_for_selector("dialog.settings", state="detached", timeout=5000)
            pg.click("#emoji-btn")
            pg.click(".emoji-tabs .custom-tab")
            pg.wait_for_selector(".emoji-section.locked .gold-locked", timeout=5000)
            shot(pg, "9904-emoji-picker-locked")
        pg.close()

    # ---- the owner grants Golden Throne (People) and Golden Outhouse (Outhouses) ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"owner-{label}", kw, OWNER)
        pg.evaluate(f"openSafetyConsole('people', null, {json.dumps(GT_ID)})")
        pg.wait_for_selector("dialog.settings .sc-golden .gold-btn", timeout=15000)
        if label == "desktop":
            pg.evaluate("document.querySelector('dialog.settings .sc-golden').scrollIntoView()")
            shot(pg, "9905-people-golden-section")
        pg.evaluate("document.querySelector('dialog.settings .sc-golden .gold-btn').click()")
        pg.wait_for_selector("dialog.gold-grant[open]", timeout=5000)
        pg.select_option("dialog.gold-grant select", "year")
        pg.fill("dialog.gold-grant input[type=text]", "Raid leader thank-you")
        shot(pg, f"9906-grant-golden-throne-{label}")
        if label == "desktop":
            pg.click("dialog.gold-grant button[type=submit]")
            pg.wait_for_selector(".toast:has-text('Golden Throne granted')", timeout=10000)
            pg.wait_for_selector("dialog.settings .sc-golden:has-text('Active until')", timeout=10000)
        else:
            pg.click("dialog.gold-grant button:has-text('Cancel')")
        pg.evaluate(f"openSafetyConsole('outhouses', null, null)")
        pg.evaluate(f"document.querySelector('dialog.settings')._draw('outhouses', {{ outhouseId: {json.dumps(GB['id'])} }})")
        pg.wait_for_selector("dialog.settings .dir-golden .gold-btn", timeout=15000)
        pg.evaluate("document.querySelector('dialog.settings .dir-golden .gold-btn').click()")
        pg.wait_for_selector("dialog.gold-grant[open]", timeout=5000)
        shot(pg, f"9907-grant-golden-outhouse-{label}")
        if label == "desktop":
            pg.select_option("dialog.gold-grant select", "forever")
            pg.click("dialog.gold-grant button[type=submit]")
            pg.wait_for_selector("dialog.settings .dir-golden:has-text('no end date')", timeout=10000)
            pg.wait_for_selector("dialog.settings .dir-head .gold-chip", timeout=10000)
            shot(pg, "9908-outhouse-detail-golden")
        else:
            pg.click("dialog.gold-grant button:has-text('Cancel')")
        pg.evaluate("document.querySelector('dialog.settings')._draw('premium')")
        pg.wait_for_selector("dialog.settings .gold-table", timeout=15000)
        shot(pg, f"9909-premium-page-{label}")
        pg.close()

    # ---- Golden Throne active ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"active-{label}", kw, GT, GA["id"])
        assert pg.evaluate("state.me.premium") is True
        user_page(pg, "golden")
        assert "Active until" in pg.inner_text("dialog.settings .gold-hero")
        if label == "desktop":
            # The free slot on the Throne Room, the Gold throne theme, profile colours, a sound.
            pg.select_option("dialog.settings #gt-slot select", GA["id"])
            pg.click("dialog.settings #gt-slot button.primary")
            pg.wait_for_selector("dialog.settings #gt-slot .gold-slot-now:has-text('since')", timeout=10000)
            pg.click("dialog.settings .gold-theme[data-theme=throne]")
            pg.wait_for_function("document.documentElement.dataset.theme === 'throne'", timeout=5000)
            pg.fill("dialog.settings #gt-colours input[type=color] >> nth=0", "#e6b422")
            pg.fill("dialog.settings #gt-colours input[type=color] >> nth=1", "#6b2bd9")
            pg.click("dialog.settings #gt-colours button.primary")
            pg.wait_for_selector(".toast:has-text('Profile colours saved')", timeout=5000)
            upload("/api/users/@me/personal-sounds", GT, {"name": "Fanfare", "emoji": "🎺"}, {"file": ("fanfare.wav", wav(1), "audio/wav")})
            upload("/api/users/@me/personal-sounds", GT, {"name": "Royal flush", "emoji": "🚽"}, {"file": ("flush.wav", wav(1), "audio/wav")})
            user_page(pg, "golden")
            pg.wait_for_selector("dialog.settings .gold-sound", timeout=10000)
            pg.click("dialog.settings .gold-sound >> nth=0 >> button:has-text('Use as entrance')")
            pg.wait_for_selector("dialog.settings .gold-sound .rp-chip:has-text('Entrance sound')", timeout=10000)
        pg.evaluate("document.querySelector('dialog.settings .settings-main').scrollTop = 0")
        shot(pg, f"9910-golden-throne-active-{label}")
        pg.evaluate("document.querySelector('dialog.settings #gt-themes').scrollIntoView()")
        shot(pg, f"9911-themes-active-{label}")
        pg.evaluate("document.querySelector('dialog.settings #gt-sounds').scrollIntoView()")
        shot(pg, f"9912-my-sounds-{label}")
        pg.evaluate("document.querySelectorAll('dialog.settings').forEach((d) => d.close())")
        pg.wait_for_selector("dialog.settings", state="detached", timeout=5000)
        # The app in the Gold throne theme, the crown on the rail.
        pg.wait_for_selector(f".guild.golden[data-id='{GA['id']}'] .rail-crown", timeout=10000)
        shot(pg, f"9913-app-gold-theme-{label}")
        if label == "desktop":
            # Own profile card: animated avatar, gradient, supporter badge.
            pg.evaluate(f"openProfile({json.dumps(GT_ID)}, document.querySelector('.me') || document.body)")
            pg.wait_for_selector(".profile-card .gold-badge", timeout=10000)
            assert pg.eval_on_selector(".profile-card .pc-avatar img", "i => i.src.includes('anim=1')"), "the profile card's avatar should move"
            shot(pg, "9914-profile-card-golden")
            pg.keyboard.press("Escape")
            # Members list: the subtle crown; avatars still until hovered.
            pg.wait_for_selector(f"#member-list .member[data-id='{GT_ID}'] .gt-mini", timeout=10000)
            assert pg.eval_on_selector(f"#member-list .member[data-id='{GT_ID}'] img", "i => i.src.includes('still=1')")
            pg.hover(f"#member-list .member[data-id='{GT_ID}']")
            pg.wait_for_function(f"document.querySelector(\"#member-list .member[data-id='{GT_ID}'] img\").src.includes('anim=1')", timeout=5000)
            # The emoji picker with the Long Drop's emoji, usable.
            pg.click("#emoji-btn")
            pg.click(".emoji-tabs .custom-tab")
            pg.wait_for_selector(".custom-pane .emoji-section:not(.locked) img[alt=':dunny:']", timeout=5000)
            shot(pg, "9915-emoji-picker-other-outhouses")
            pg.click(".custom-pane img[alt=':dunny:']")
            pg.press("#composer-input", "Enter")
            pg.wait_for_selector(".msg img.custom-emoji[alt=':dunny:'], .message img.custom-emoji", timeout=10000)
            # The soundboard: My sounds.
            pg.evaluate("openSoundboard(document.querySelector('#composer-input'))")
            pg.wait_for_selector(".sb-board .sb-sec[data-sec='mine'] .sb-tile.sb-personal", timeout=10000)
            shot(pg, "9916-soundboard-my-sounds")
            pg.keyboard.press("Escape")
            # The long-message counter.
            pg.fill("#composer-input", "x" * 3800)
            pg.wait_for_selector("#composer-count:not([hidden])", timeout=5000)
            assert "4,000" in pg.inner_text("#composer-count")
            shot(pg, "9917-composer-counter")
            pg.fill("#composer-input", "")
        pg.close()

    # ---- Golden Outhouse: settings page and the vanity link ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = signed_in(f"go-{label}", kw, PL, GB["id"])
        pg.evaluate("openSettings('golden')")
        pg.wait_for_selector("dialog.settings .gold-hero.active", timeout=15000)
        shot(pg, f"9918-golden-outhouse-{label}")
        pg.evaluate("openSettings('invites')")
        pg.wait_for_selector("dialog.settings .gold-vanity input", timeout=15000)
        if label == "desktop":
            pg.fill("dialog.settings .gold-vanity input", "Bad--Name")
            pg.wait_for_selector("dialog.settings .gold-vanity .error:has-text('single hyphens')", timeout=5000)
            pg.fill("dialog.settings .gold-vanity input", f"long-drop-{suffix}")
            pg.click("dialog.settings .gold-vanity button.primary")
            pg.wait_for_selector("dialog.settings .gold-vanity .gold-vanity-now:has-text('/invite/long-drop')", timeout=10000)
        shot(pg, f"9919-vanity-setting-{label}")
        pg.close()
    # Not premium: the Golden Outhouse page and vanity box, locked (Throne Room
    # is premium through Goldie's slot, so use a fresh outhouse).
    GC = api("POST", "/api/guilds", {"name": f"Back Shed {suffix}"}, PL)
    api("POST", f"/api/guilds/{GC['id']}/channels", {"name": "general"}, PL)
    pg = signed_in("go-locked", DESKTOP, PL, GC["id"])
    pg.evaluate("openSettings('golden')")
    pg.wait_for_selector("dialog.settings .gold-hero:not(.active)", timeout=15000)
    shot(pg, "9920-golden-outhouse-locked")
    pg.evaluate("openSettings('invites')")
    pg.wait_for_selector("dialog.settings .gold-vanity.locked", timeout=15000)
    shot(pg, "9921-vanity-locked")
    pg.close()

    # ---- the invite page with the vanity link (signed out, desktop and phone) ----
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        pg = browser.new_page(**kw)
        watch(pg, f"invite-{label}")
        pg.goto(f"{BASE}/invite/long-drop-{suffix}")
        pg.wait_for_selector("#inv-golden:not([hidden])", timeout=10000)
        assert pg.get_attribute("body", "data-state") == "valid"
        shot(pg, f"9922-invite-vanity-{label}")
        pg.close()
    # Paused: Golden Outhouse revoked.
    api("POST", f"/api/safety/outhouses/{GB['id']}/premium/revoke", {}, OWNER)
    api("POST", "/api/premium/sweep", None, OWNER, ok=False)
    pg = browser.new_page(**DESKTOP)
    watch(pg, "invite-paused")
    pg.goto(f"{BASE}/invite/long-drop-{suffix}")
    pg.wait_for_selector("text=This link isn't active right now", timeout=10000)
    assert pg.get_attribute("body", "data-state") == "paused"
    shot(pg, "9923-invite-vanity-paused")
    pg.close()

    # ---- the info panel (Throne Room: premium through the slot) ----
    pg = signed_in("info", DESKTOP, PL, GA["id"])
    pg.evaluate(f"openGuildInfo({json.dumps(GA['id'])})")
    pg.wait_for_selector("dialog.guild-info .gold-badge", timeout=10000)
    shot(pg, "9924-info-panel-golden")
    pg.close()

    browser.close()

# Leave things tidy for a second run.
api("POST", f"/api/safety/users/{GT_ID}/premium/revoke", {}, OWNER, ok=False)
if errors:
    print("\n".join(errors))
    sys.exit(1)
print("premium UI test passed")
