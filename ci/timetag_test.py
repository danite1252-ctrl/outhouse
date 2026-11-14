"""Time tags (<t:UNIX:STYLE>) in headless Chrome: the same message reads
differently in Sydney and London, every style renders, "R" keeps counting,
code stays literal, bad tags stay text, previews spell them out, the hover
card names your zone and UTC, the composer's clock picker inserts the right
tag (a bottom sheet on phones, in threads too), and a raid's ⋯ menu copies
its time tag.

Fails on any JavaScript error (and runs with the real Content-Security-Policy).
Screenshots go to $SHOTS as 30-*.png.
"""

import datetime as dt
import json
import os
import re
import sys
import time
import urllib.request
from zoneinfo import ZoneInfo

from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE", "http://127.0.0.1:8080")
SHOTS = os.environ.get("SHOTS", "/tmp/ui-shots")
os.makedirs(SHOTS, exist_ok=True)
errors = []


def api(method, path, body=None, token=None, xff=None):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Content-Type", "application/json")
    if xff:
        req.add_header("X-Forwarded-For", xff)
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


def register(name, display, ip):
    # Its own addresses: sign-ups are limited per address, and the other UI tests use 127.0.0.1's.
    return api("POST", "/api/auth/register", {"username": name, "email": "someone@example.test", "password": "password123", "birth_date": "1990-01-01", "display_name": display}, xff=ip)


def norm(s):
    return (s or "").replace(" ", " ").replace(" ", " ").strip()


wait_for_server()
suffix = str(int(time.time()))[-6:]
tess = register(f"tess{suffix}", "Tess", "198.18.30.1")
ulf = register(f"ulf{suffix}", "Ulf", "198.18.30.2")
TOKEN = tess["token"]
guild = api("POST", "/api/guilds", {"name": "Time Zone Raiders"}, TOKEN)
G = guild["id"]
chan = api("POST", f"/api/guilds/{G}/channels", {"name": "raid-chat"}, TOKEN)
C = chan["id"]
code = api("POST", f"/api/guilds/{G}/invites", None, TOKEN)["code"]
api("POST", f"/api/invites/{code}", None, ulf["token"])

TS = 1767259800  # Thu 1 Jan 2026 09:30 UTC: 8:30 pm in Sydney (AEDT), 9:30 am in London
STYLES = "tTdDfF"
for s in STYLES:
    api("POST", f"/api/channels/{C}/messages", {"content": f"style {s}: <t:{TS}:{s}>"}, TOKEN)
api("POST", f"/api/channels/{C}/messages", {"content": f"default: <t:{TS}>"}, TOKEN)
soon = int(time.time()) + 2 * 3600 + 300
rel = api("POST", f"/api/channels/{C}/messages", {"content": f"pull timer <t:{soon}:R> **bold <t:{TS}:t>**"}, TOKEN)
api("POST", f"/api/channels/{C}/messages", {"content": f"literal `<t:{TS}:F>` and bad <t:abc:f> <t:{TS}:x> <t:99999999999999:f>"}, TOKEN)
api("POST", f"/api/channels/{C}/messages", {"content": "raid moved to tonight", "reply_to_id": rel["id"]}, ulf["token"])
api("POST", f"/api/channels/{C}/messages", {"content": f"reply check", "reply_to_id": api("POST", f"/api/channels/{C}/messages", {"content": f"see you at <t:{TS}:t>"}, TOKEN)["id"]}, ulf["token"])
start = time.strftime("%Y-%m-%dT10:00:00Z", time.gmtime(time.time() + 3 * 86400))
raid = api("POST", f"/api/channels/{C}/events", {"title": "Blackwing Lair", "description": "Bring fire resist.", "starts_at": start}, TOKEN)
RAID_TS = int(dt.datetime.strptime(start, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc).timestamp())
# A thread to check its composer has the clock too.
tmsg = api("POST", f"/api/channels/{C}/messages", {"content": "Thread for the raid plan"}, TOKEN)
api("POST", f"/api/channels/{C}/threads", {"name": "Raid plan", "message_id": tmsg["id"]}, TOKEN)
api("POST", f"/api/channels/{C}/messages", {"content": "last one"}, TOKEN)

EXPECT = {
    "Australia/Sydney": {"t": "8:30 PM", "T": "8:30:00 PM", "d": "1/1/2026", "D": "January 1, 2026", "f": ["January 1, 2026", "8:30 PM"], "F": ["Thursday", "January 1, 2026", "8:30 PM"]},
    "Europe/London": {"t": "9:30 AM", "T": "9:30:00 AM", "d": "1/1/2026", "D": "January 1, 2026", "f": ["January 1, 2026", "9:30 AM"], "F": ["Thursday", "January 1, 2026", "9:30 AM"]},
}


def watch(page, label):
    page.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
    page.on("console", lambda m: m.type == "error" and "favicon" not in m.text and "Failed to load resource" not in m.text
            and errors.append(f"[{label}] console: {m.text}"))


def open_app(browser, label, tz, viewport, **kw):
    ctx = browser.new_context(viewport=viewport, timezone_id=tz, locale="en-US", **kw)
    pg = ctx.new_page()
    watch(pg, label)
    pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(TOKEN)}); localStorage.setItem('outhouse_guild', {json.dumps(G)}); localStorage.setItem('outhouse_channel_{G}', {json.dumps(C)});")
    pg.goto(BASE + "/")
    pg.wait_for_selector("#messages .msg-text:has-text('last one')", timeout=20000)
    return ctx, pg


def styled(pg, style):
    """The chip in "style X: …" (case matters: t and T are different styles)."""
    return pg.locator("#messages .msg-text").filter(has_text=re.compile(f"style {style}:")).locator("time.timetag").first


def chip_text(pg, style):
    return norm(styled(pg, style).inner_text())


def check_styles(pg, tz):
    for s, want in EXPECT[tz].items():
        got = chip_text(pg, s)
        for part in (want if isinstance(want, list) else [want]):
            assert part in got, f"{tz} style {s}: {got!r} should contain {part!r}"
        assert styled(pg, s).get_attribute("data-style") == s
    default = norm(pg.locator("#messages .msg-text:has-text('default:') time.timetag").first.inner_text())
    assert default == chip_text(pg, "f"), (default, chip_text(pg, "f"))


def tip_text(pg, chip, how="hover", tries=6):
    """Hovers (or taps) a chip and reads its card. The list can still be
    redrawing just after it opens (raid cards, presence), which replaces the
    chip under the pointer, so try again until the card stays."""
    for i in range(tries):
        chip.scroll_into_view_if_needed()
        if how == "tap":
            if pg.locator(".tt-tip").count():
                pg.mouse.click(2, 300)
            chip.tap()
        else:
            pg.mouse.move(2, 2)
            chip.hover()
        try:
            pg.wait_for_selector(".tt-tip", timeout=3000)
            pg.wait_for_timeout(300)
            return norm(pg.inner_text(".tt-tip", timeout=1000))
        except Exception:
            if i == tries - 1:
                raise
            pg.wait_for_timeout(400)


def copy_to_preview(name):
    out = "/mnt/user-data/outputs/preview"
    try:
        os.makedirs(out, exist_ok=True)
        with open(f"{SHOTS}/{name}.png", "rb") as a, open(f"{out}/{name}.png", "wb") as b:
            b.write(a.read())
    except OSError:
        pass


def shot(pg, name, **kw):
    pg.screenshot(path=f"{SHOTS}/{name}.png", **kw)
    copy_to_preview(name)


with sync_playwright() as p:
    browser = p.chromium.launch()

    # ---- Sydney, desktop ----
    syd_ctx, syd = open_app(browser, "sydney", "Australia/Sydney", {"width": 1280, "height": 800},
                            permissions=["clipboard-read", "clipboard-write"])
    check_styles(syd, "Australia/Sydney")

    # Relative: "in 2 hours", and it keeps up (the 30-second refresh).
    relchip = syd.locator("#messages .msg-text:has-text('pull timer') time.timetag.tt-rel").first
    assert norm(relchip.inner_text()) == "in 2 hours", relchip.inner_text()
    syd.evaluate("""() => {
      const c = document.querySelector('#messages time.timetag.tt-rel');
      c.dataset.ts = String(Math.floor(Date.now() / 1000) - 3 * 86400);
      document.dispatchEvent(new Event('visibilitychange'));
    }""")
    syd.wait_for_function("() => document.querySelector('#messages time.timetag.tt-rel').textContent === '3 days ago'", timeout=5000)
    syd.evaluate(f"() => {{ const c = document.querySelector('#messages time.timetag.tt-rel'); c.dataset.ts = '{soon}'; document.dispatchEvent(new Event('visibilitychange')); }}")
    # Bold around a tag still makes a chip.
    assert "8:30 PM" in norm(syd.locator("#messages .msg-text:has-text('pull timer') strong time.timetag").inner_text())

    # Code stays literal; bad tags stay text.
    lit = syd.locator("#messages .msg-text:has-text('literal')").first
    assert lit.locator("code").inner_text() == f"<t:{TS}:F>"
    assert lit.locator("time.timetag").count() == 0
    assert "<t:abc:f>" in lit.inner_text() and f"<t:{TS}:x>" in lit.inner_text() and "<t:99999999999999:f>" in lit.inner_text()

    # Reply previews spell tags out (and never show the raw tag).
    ref = norm(syd.locator(".reply-ref:has-text('see you at')").first.inner_text())
    assert "see you at 8:30 PM" in ref and "<t:" not in ref, ref

    # The hover card: full date and time, your zone, UTC.
    tip = tip_text(syd, styled(syd, "F"))
    assert "Australia/Sydney" in tip and "09:30 UTC" in tip and "8:30 PM" in tip and "Thursday" in tip, tip
    shot(syd, "30-timetags-desktop-sydney")
    syd.mouse.move(5, 5)
    syd.wait_for_selector(".tt-tip", state="detached", timeout=5000)

    # The composer's clock picker: date + time + style with previews → Insert.
    syd.click("#time-btn")
    syd.wait_for_selector(".popover.tt-pop", timeout=5000)
    assert syd.locator(".tt-pop input[type=date]").count() == 1 and syd.locator(".tt-pop input[type=time]").count() == 1
    # Prefilled with the next round half-hour.
    t0 = syd.input_value(".tt-pop .tt-time")
    assert t0.endswith(":00") or t0.endswith(":30"), t0
    syd.fill(".tt-pop .tt-date", "2026-03-14")
    syd.fill(".tt-pop .tt-time", "19:30")
    want_ts = int(dt.datetime(2026, 3, 14, 19, 30, tzinfo=ZoneInfo("Australia/Sydney")).timestamp())
    labels = [norm(x) for x in syd.locator(".tt-pop .tt-style option").all_inner_texts()]
    assert len(labels) == 7 and any("7:30 PM" in x for x in labels) and any("Saturday" in x for x in labels), labels
    assert syd.input_value(".tt-pop .tt-style") == "f"
    assert syd.inner_text(".tt-pop .tt-raw") == f"<t:{want_ts}:f>"
    syd.select_option(".tt-pop .tt-style", "F")
    assert syd.inner_text(".tt-pop .tt-raw") == f"<t:{want_ts}:F>"
    assert "Saturday" in norm(syd.inner_text(".tt-pop .tt-preview time.timetag"))
    shot(syd, "30-timetag-picker-desktop")
    syd.fill("#composer-input", "Raid moved to")
    syd.click(".tt-pop .tt-insert")
    syd.wait_for_selector(".popover.tt-pop", state="detached", timeout=5000)
    assert syd.input_value("#composer-input") == f"Raid moved to <t:{want_ts}:F>", syd.input_value("#composer-input")
    syd.press("#composer-input", "Enter")
    syd.wait_for_selector(f"#messages time.timetag[data-ts='{want_ts}']", timeout=10000)
    assert "Saturday" in norm(syd.inner_text(f"#messages time.timetag[data-ts='{want_ts}']"))

    # The thread composer has the clock too.
    syd.click("#messages .thread-link")
    syd.wait_for_selector(".thread-composer .thread-time-btn", timeout=10000)
    syd.click(".thread-composer .thread-time-btn")
    syd.wait_for_selector(".popover.tt-pop", timeout=5000)
    syd.select_option(".tt-pop .tt-style", "t")
    syd.click(".tt-pop .tt-insert")
    tval = syd.input_value(".thread-composer textarea")
    assert tval.startswith("<t:") and tval.endswith(":t>"), tval
    syd.fill(".thread-composer textarea", "")
    syd.keyboard.press("Escape")

    # The raid's ⋯ menu copies <t:start:F>.
    syd.locator(".event-card .rc-menu").first.scroll_into_view_if_needed()
    syd.click(".event-card .rc-menu")
    syd.click(".menu button:has-text('Copy time tag'), .menu [role=menuitem]:has-text('Copy time tag')")
    syd.wait_for_function("() => true")
    clip = syd.evaluate("navigator.clipboard.readText()")
    assert clip == f"<t:{RAID_TS}:F>", clip

    # ---- London, desktop: the same tags, other times ----
    lon_ctx, lon = open_app(browser, "london", "Europe/London", {"width": 1280, "height": 800})
    check_styles(lon, "Europe/London")
    assert chip_text(lon, "t") != chip_text(syd, "t")
    assert "Europe/London" in tip_text(lon, styled(lon, "F"))
    shot(lon, "30-timetags-desktop-london")
    lon_ctx.close()

    # ---- Phone (London): tap for the card, the picker is a bottom sheet ----
    ph_ctx, ph = open_app(browser, "phone", "Europe/London", {"width": 390, "height": 844}, is_mobile=True, has_touch=True)
    check_styles(ph, "Europe/London")
    # (F wraps onto its own line, clear of the message's action bar that a tap shows.)
    pchip = styled(ph, "F")
    assert "Europe/London" in tip_text(ph, pchip, how="tap")
    shot(ph, "30-timetags-phone-tip")
    pchip.tap()
    ph.wait_for_selector(".tt-tip", state="detached", timeout=5000)
    # On phones the clock lives in the ＋ menu so the message box keeps its room.
    assert not ph.locator("#time-btn").is_visible()
    ph.locator("#attach-btn").tap()
    ph.locator(".menu [role=menuitem]:has-text('Insert a time')").tap()
    ph.wait_for_selector(".popover.tt-pop.tt-sheet", timeout=5000)
    ph.wait_for_timeout(400)  # the sheet slides up
    box = ph.locator(".popover.tt-pop").bounding_box()
    assert box["width"] >= 389 and abs(box["y"] + box["height"] - 844) < 2, box
    ph.fill(".tt-pop .tt-date", "2026-07-04")
    ph.fill(".tt-pop .tt-time", "20:00")
    ph.select_option(".tt-pop .tt-style", "R")
    want_ph = int(dt.datetime(2026, 7, 4, 20, 0, tzinfo=ZoneInfo("Europe/London")).timestamp())
    assert ph.inner_text(".tt-pop .tt-raw") == f"<t:{want_ph}:R>"
    shot(ph, "30-timetag-picker-phone")
    ph.locator(".tt-pop .tt-insert").tap()
    assert ph.input_value("#composer-input") == f"<t:{want_ph}:R>"
    ph_ctx.close()
    syd_ctx.close()
    browser.close()

if errors:
    print("\n".join(errors))
    sys.exit("JavaScript errors in the time tag test")
print("time tag UI test passed")
