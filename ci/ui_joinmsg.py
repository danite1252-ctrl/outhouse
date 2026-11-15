"""UI test for join and leave messages (web/joinmsg.js): a join line drawn as
a one-liner with the member's name as a clickable mention (opens their card),
the "Wave" button sending a 👋 reply, a leave line and a bot line, no Edit for
the member, and the Welcome settings on Outhouse settings → Overview (switch
off, pick a channel). Desktop and phone screenshots (90-*).

ci/ui_test.py calls run(); it can also run on its own against a local server
(see the bottom).
"""

import json
import time
import urllib.request

PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
DESKTOP = {"viewport": {"width": 1280, "height": 860}}


def run(browser, base, shots, watch):
    def api(method, path, body=None, token=None, xff=None):
        req = urllib.request.Request(base + path, method=method)
        req.add_header("Content-Type", "application/json")
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        if xff:
            req.add_header("X-Forwarded-For", xff)
        data = json.dumps(body).encode() if body is not None else None
        with urllib.request.urlopen(req, data) as res:
            text = res.read().decode()
            return json.loads(text) if text else None

    run_id = str(int(time.time()))[-6:]
    n = [0]

    def reg(name, display):
        n[0] += 1
        return api("POST", "/api/auth/register", {"username": f"{name}{run_id}", "email": f"{name}{run_id}@example.test", "password": "password123",
                                                  "birth_date": "1990-01-01", "display_name": display}, xff=f"198.20.{int(run_id) % 200 + 30}.{n[0]}")

    owner, bob, cat, dee = reg("jowner", "Porchlight"), reg("jbob", "Bob Dunny"), reg("jcat", "Cat"), reg("jdee", "Dee")
    T = owner["token"]
    g = api("POST", "/api/guilds", {"name": "The Long Drop"}, T)
    porch = [c for c in api("GET", f"/api/guilds/{g['id']}/channels", None, T) if c["kind"] == 0][0]
    api("PUT", f"/api/guilds/{g['id']}/welcome", {"leave_messages": True}, T)
    api("POST", f"/api/channels/{porch['id']}/messages", {"content": "Doors open, mind the step."}, T)
    for u in (bob, cat, dee):
        code = api("POST", f"/api/guilds/{g['id']}/invites", None, T)["code"]
        api("POST", f"/api/invites/{code}", None, u["token"])
    api("POST", f"/api/channels/{porch['id']}/messages", {"content": "g'day all"}, cat["token"])
    api("POST", f"/api/guilds/{g['id']}/leave", None, dee["token"])
    api("POST", f"/api/guilds/{g['id']}/bots", {"name": "Plunger Bot"}, T)
    msgs = api("GET", f"/api/channels/{porch['id']}/messages?limit=50", None, T)
    sysmsgs = [m for m in msgs if m.get("system")]
    assert len(sysmsgs) == 5, [m["content"] for m in msgs]
    bob_join = next(m for m in sysmsgs if m["author_id"] == bob["user"]["id"])
    cat_join = next(m for m in sysmsgs if m["author_id"] == cat["user"]["id"] and m["system"]["kind"] == "member_join")
    api("PUT", f"/api/channels/{porch['id']}/messages/{bob_join['id']}/reactions/%F0%9F%91%8B", None, cat["token"])

    def page_for(token, label, kind):
        pg = browser.new_page(**kind)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(g['id'])}); localStorage.setItem('outhouse_channel_{g['id']}', {json.dumps(porch['id'])});")
        pg.goto(base + "/")
        pg.wait_for_selector(f"#msg-{bob_join['id']}.sys-msg", timeout=15000)
        return pg

    # ---- Desktop, as the owner: the lines, the wave button, the member card ----
    pg = page_for(T, "joinmsg-desktop", DESKTOP)
    line = f"#msg-{bob_join['id']}"
    text = pg.inner_text(f"{line} .sys-text")
    assert "@Bob Dunny" in text and "{name}" not in text, text
    assert pg.query_selector(f"{line} .msg-author") is None and pg.query_selector(f"{line} .avatar") is None
    assert pg.query_selector_all("#messages .sys-msg.sys-member_leave") and pg.query_selector_all("#messages .sys-msg.sys-bot_join")
    # The owner can delete it but nobody can edit it.
    pg.hover(line)
    assert pg.query_selector(f"{line} .msg-actions button[title='Edit']") is None
    assert pg.query_selector(f"{line} .msg-actions button[title='Delete']") is not None
    pg.mouse.move(5, 5)
    pg.wait_for_timeout(300)
    pg.screenshot(path=f"{shots}/90-desktop-join-messages.png")
    # Wave at Cat: a 👋 reply, and the button goes away.
    wave = f"#msg-{cat_join['id']} .sys-wave"
    pg.wait_for_selector(wave)
    pg.click(wave)
    pg.wait_for_selector(f"#messages .reply-ref:has-text('Cat')", timeout=10000)
    pg.wait_for_selector(wave, state="detached", timeout=10000)
    # The name opens their card.
    pg.click(f"{line} .sys-name")
    pg.wait_for_selector(".popover:has-text('Bob Dunny')", timeout=10000)
    pg.wait_for_timeout(300)
    pg.screenshot(path=f"{shots}/90-desktop-wave-and-card.png")
    pg.keyboard.press("Escape")

    # ---- Settings: Overview → Welcome ----
    pg.evaluate("openSettings('overview')")
    pg.wait_for_selector("#welcome-settings .jm-channel", timeout=10000)
    pg.wait_for_timeout(300)
    pg.evaluate("document.querySelector('#welcome-settings').scrollIntoView({block: 'start'})")
    pg.wait_for_timeout(300)
    pg.screenshot(path=f"{shots}/90-desktop-welcome-settings.png")
    mat = api("POST", f"/api/guilds/{g['id']}/channels", {"name": "welcome-mat"}, T)
    pg.evaluate("document.querySelectorAll('dialog[open]').forEach((d) => d.close())")
    pg.wait_for_function("id => state.channels.some((c) => c.id === id)", arg=mat["id"], timeout=10000)
    pg.evaluate("openSettings('overview')")  # again, with the new channel in the list
    pg.wait_for_selector(f"#welcome-settings .jm-channel option[value='{mat['id']}']", state="attached", timeout=10000)
    pg.select_option("#welcome-settings .jm-channel", mat["id"])
    pg.wait_for_selector("#welcome-settings .ec-hint:has-text('#welcome-mat')", timeout=10000)
    assert api("GET", f"/api/guilds/{g['id']}/welcome", None, T)["channel_id"] == mat["id"]
    pg.click("#welcome-settings .jm-row:first-of-type .am-switch")
    for _ in range(40):
        if not api("GET", f"/api/guilds/{g['id']}/welcome", None, T)["join_messages"]:
            break
        time.sleep(0.25)
    else:
        raise AssertionError("switching join messages off didn't save")
    api("PUT", f"/api/guilds/{g['id']}/welcome", {"join_messages": True, "channel_id": None}, T)
    pg.close()

    # ---- Phone, as Bob: his own line has no Edit, and the wave button is for others ----
    ph = page_for(bob["token"], "joinmsg-phone", PHONE)
    assert ph.query_selector(f"#msg-{bob_join['id']} .sys-wave") is None
    assert ph.query_selector(f"#msg-{cat_join['id']} .sys-wave") is not None
    ph.evaluate("id => document.getElementById('msg-' + id).scrollIntoView({block: 'center'})", bob_join["id"])
    ph.wait_for_timeout(400)
    ph.screenshot(path=f"{shots}/90-phone-join-messages.png")
    ph.close()


if __name__ == "__main__":
    # Local run: BASE=http://127.0.0.1:8381 SHOTS=/tmp/shots python3 ci/ui_joinmsg.py
    import os
    from playwright.sync_api import sync_playwright
    errors = []

    def watch(pg, label):
        pg.on("pageerror", lambda e: errors.append(f"[{label}] page error: {e}"))
        pg.on("console", lambda m: m.type == "error" and "favicon" not in m.text and "Failed to load resource" not in m.text
              and errors.append(f"[{label}] console: {m.text}"))

    shots = os.environ.get("SHOTS", "/tmp/ui-shots")
    os.makedirs(shots, exist_ok=True)
    with sync_playwright() as p:
        b = p.chromium.launch()
        run(b, os.environ.get("BASE", "http://127.0.0.1:8080"), shots, watch)
        b.close()
    print("\n".join(errors) or "no errors")
