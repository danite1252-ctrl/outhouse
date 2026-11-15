"""UI test for channel categories and moving channels (web/categories.js and
renderChannels in web/app.js): the classic "TEXT CHANNELS" / "VOICE" look
with no categories, making a category from the ＋ menu and a channel in it
from the category's ＋ (Category picker), drag and drop on a computer
(reordering in a category, into another category with the "Sync
permissions?" offer, a text channel dropped among voice channels snapping to
the text channels, reordering categories, a voice channel taking its people
with it), the list updating live for someone else, collapsing (remembered
after a reload, still showing the open channel), Synced / Not synced on the
Permissions tab, members (no dragging, no move menus, private categories
hidden), "View outhouse as role", and the phone menus (Move up / Move down /
Move to category). Desktop and phone screenshots (95-*).

ci/ui_test.py calls run(); it can also run on its own against a local server
(see the bottom).
"""

import json
import os
import time
import urllib.request

PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
DESKTOP = {"viewport": {"width": 1280, "height": 800}}


def run(browser, base, shots, watch, db_url=None):
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
                                                  "birth_date": "1990-01-01", "display_name": display}, xff=f"198.19.{int(run_id) % 200 + 20}.{n[0] + 100}")

    owner, member = reg("catown", "Cat Boss"), reg("catmem", "Cat Pal")
    T, TM = owner["token"], member["token"]
    g = api("POST", "/api/guilds", {"name": "Category Corner"}, T)
    G = g["id"]
    code = api("POST", f"/api/guilds/{G}/invites", None, T)["code"]
    api("POST", f"/api/invites/{code}", None, TM)
    chans = api("GET", f"/api/guilds/{G}/channels", None, T)
    porch = next(c for c in chans if c["kind"] == 0)
    drop = next(c for c in chans if c["kind"] == 2)
    api("POST", f"/api/channels/{porch['id']}/messages", {"content": "welcome to the corner"}, T)

    def listing(token=T):
        cs = api("GET", f"/api/guilds/{G}/channels", None, token)
        names = {c["id"]: c["name"] for c in cs}
        return [(c["kind"], c["name"], names.get(c["parent_id"]) if c.get("parent_id") else None) for c in cs], cs

    def page(token, label, opts=DESKTOP, channel=None):
        pg = browser.new_page(**opts)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(G)});"
                           + (f"localStorage.setItem('outhouse_channel_{G}', {json.dumps(channel)});" if channel else ""))
        pg.goto(base + "/")
        pg.wait_for_selector("#channel-tree .channel.text", timeout=15000)
        pg.wait_for_timeout(400)
        return pg

    def tree_order(pg):
        # [(category or "", kind, name)] in the order drawn.
        return pg.evaluate("""() => [...document.querySelectorAll('#channel-tree .ch-item')].map((n) => [
            n.closest('.ch-category')?.querySelector('.cat-name')?.textContent || '',
            n.classList.contains('voice-item') ? 'voice' : 'text',
            state.channels.find((c) => c.id === n.dataset.id)?.name])""")

    def wait_tree(pg, want, what, timeout=10):
        end = time.time() + timeout
        got = None
        while time.time() < end:
            got = tree_order(pg)
            if want(got):
                return got
            pg.wait_for_timeout(150)
        raise AssertionError(f"{what}: {got}")

    def item(name):
        return f"#channel-tree .ch-item:has(> .channel.text .cname:text-is('{name}'))"

    def voice_item(name):
        return f"#channel-tree .ch-item.voice-item:has(> .channel:has-text('{name}'))"

    def drag(pg, src, dst, where="top", shot=None):
        """Drags src onto the top or bottom edge of dst with the mouse (HTML5 drag and drop)."""
        pg.wait_for_timeout(400)  # let any redraw from the last move land first
        s = pg.locator(src).first.bounding_box()
        d = pg.locator(dst).first.bounding_box()
        y = d["y"] + (4 if where == "top" else d["height"] - 4 if where == "bottom" else d["height"] / 2)
        pg.mouse.move(s["x"] + 30, s["y"] + s["height"] / 2)
        pg.mouse.down()
        pg.mouse.move(s["x"] + 34, s["y"] + s["height"] / 2 + 3, steps=3)
        pg.mouse.move(d["x"] + 40, y, steps=8)
        pg.wait_for_timeout(120)
        if shot:
            pg.screenshot(path=shot)
        pg.mouse.up()
        pg.wait_for_timeout(300)

    # ---- No categories: the classic look ----
    dp = page(T, "categories-desktop", channel=porch["id"])
    labels = dp.eval_on_selector_all("#channel-tree .section-label", "ns => ns.map((n) => n.firstChild.textContent.trim())")
    assert labels == ["TEXT CHANNELS", "VOICE"], labels
    assert dp.locator("#channel-tree .category-head").count() == 0
    assert dp.locator("#channel-tree #new-channel").is_visible()
    dp.screenshot(path=f"{shots}/95-desktop-no-categories.png")
    mp = page(TM, "categories-member", channel=porch["id"])
    assert mp.locator("#channel-add").is_hidden(), "members got the create button"
    assert mp.locator("#channel-tree [draggable='true']").count() == 0, "members can drag"

    # ---- Make a category from the ＋ menu ----
    dp.click("#channel-add")
    dp.click(".menu button:has-text('Create category')")
    dp.fill("dialog[open] input", "Raid nights")
    dp.click("dialog[open] button.primary")
    dp.wait_for_selector("#channel-tree .category-head:has-text('RAID NIGHTS')", timeout=10000)
    assert dp.locator("#channel-tree .section-label").count() == 0, "the classic headers stayed with a category"
    raids = next(c for c in listing()[1] if c["kind"] == 4)

    # A channel in it from the category's ＋ (the Category picker starts on it).
    dp.hover("#channel-tree .category-head:has-text('RAID NIGHTS')")
    dp.click("#channel-tree .category-head:has-text('RAID NIGHTS') .cat-add")
    dp.wait_for_selector("dialog[open] select.cc-category")
    assert dp.eval_on_selector("dialog[open] select.cc-category", "s => s.value") == raids["id"]
    assert dp.locator("dialog[open] .cc-sync").is_visible()
    dp.fill("dialog[open] input:not([type])", "signups")
    dp.screenshot(path=f"{shots}/95-desktop-create-in-category.png")
    dp.click("dialog[open] button.primary")
    dp.wait_for_selector("#channel-tree .ch-category .channel.text:has-text('signups')", timeout=10000)
    dp.wait_for_selector("#channel-tree .channel.text.active:has-text('signups')", timeout=10000)

    strats = api("POST", f"/api/guilds/{G}/channels", {"name": "strats", "parent_id": raids["id"]}, T)
    comms = api("POST", f"/api/guilds/{G}/channels", {"name": "Raid comms", "kind": 2, "parent_id": raids["id"]}, T)
    social = api("POST", f"/api/guilds/{G}/channels", {"name": "Social", "kind": 4}, T)
    chat = api("POST", f"/api/guilds/{G}/channels", {"name": "chat", "parent_id": social["id"]}, T)
    # Social's permissions differ from front-porch's (no @everyone pings there).
    api("PUT", f"/api/channels/{social['id']}/permissions/{G}", {"kind": "role", "allow": "0", "deny": "64"}, T)
    api("PUT", f"/api/channels/{chat['id']}/permissions/{G}", {"kind": "role", "allow": "0", "deny": "64"}, T)
    want = [["", "text", "front-porch"], ["", "voice", "The Long Drop"],
            ["RAID NIGHTS", "text", "signups"], ["RAID NIGHTS", "text", "strats"], ["RAID NIGHTS", "voice", "Raid comms"],
            ["SOCIAL", "text", "chat"]]
    wait_tree(dp, lambda t: t == want, "owner's list after making channels")
    wait_tree(mp, lambda t: t == want, "member's list after making channels")
    dp.screenshot(path=f"{shots}/95-desktop-categories.png")

    # ---- Drag and drop ----
    assert dp.locator("#channel-tree .ch-item[draggable='true']").count() == 6
    # Reorder inside a category: strats above signups (the drop line shows while dragging).
    drag(dp, item("strats"), item("signups"), "top", shot=f"{shots}/95-desktop-dragging.png")
    want = [["", "text", "front-porch"], ["", "voice", "The Long Drop"],
            ["RAID NIGHTS", "text", "strats"], ["RAID NIGHTS", "text", "signups"], ["RAID NIGHTS", "voice", "Raid comms"],
            ["SOCIAL", "text", "chat"]]
    wait_tree(dp, lambda t: t == want, "reorder in a category")
    wait_tree(mp, lambda t: t == want, "member saw the reorder live")
    assert [x[1] for x in listing()[0] if x[2] == "Raid nights"] == ["strats", "signups", "Raid comms"]

    # Into another category: front-porch under chat in SOCIAL; Sync is offered.
    drag(dp, item("front-porch"), item("chat"), "bottom")
    wait_tree(dp, lambda t: ["SOCIAL", "text", "front-porch"] in t and t.index(["SOCIAL", "text", "front-porch"]) == t.index(["SOCIAL", "text", "chat"]) + 1,
              "front-porch into SOCIAL")
    dp.wait_for_selector(".cat-sync-offer:has-text('#front-porch')", timeout=10000)
    dp.screenshot(path=f"{shots}/95-desktop-sync-offer.png")
    dp.click(".cat-sync-offer button.primary")
    dp.wait_for_selector(".cat-sync-offer", state="detached")
    end = time.time() + 10
    while time.time() < end and not next(c for c in listing()[1] if c["id"] == porch["id"])["permissions_synced"]:
        time.sleep(0.2)
    assert next(c for c in listing()[1] if c["id"] == porch["id"])["permissions_synced"], "Sync didn't copy the permissions"

    # A text channel dropped on a voice channel goes with the text channels there.
    drag(dp, item("chat"), voice_item("Raid comms"), "middle")
    wait_tree(dp, lambda t: t.index(["RAID NIGHTS", "text", "chat"]) == t.index(["RAID NIGHTS", "voice", "Raid comms"]) - 1
              if ["RAID NIGHTS", "text", "chat"] in t else False, "text among voice snaps to text")
    dp.click(".cat-sync-offer button.ghost") if dp.locator(".cat-sync-offer").count() else None

    # A voice channel moves with the people in it.
    dp.evaluate(f"""() => {{ state.voiceStates.set({json.dumps(member['user']['id'])}, {{ user_id: {json.dumps(member['user']['id'])}, channel_id: {json.dumps(comms['id'])}, muted: false, deafened: false }}); renderChannels(); }}""")
    dp.wait_for_selector(f"{voice_item('Raid comms')} .voice-user")
    drag(dp, voice_item("Raid comms"), voice_item("The Long Drop"), "top")
    wait_tree(dp, lambda t: t[0] == ["", "voice", "Raid comms"], "voice channel to the top")
    assert dp.locator(f"{voice_item('Raid comms')} .voice-user").count() == 1, "the voice user didn't move with the channel"

    # Reorder categories: SOCIAL above RAID NIGHTS.
    drag(dp, "#channel-tree .category-head:has-text('SOCIAL')", "#channel-tree .category-head:has-text('RAID NIGHTS')", "top")
    wait_tree(dp, lambda t: [c for c, _, _ in t if c][:1] == ["SOCIAL"], "categories reordered")
    assert [x[1] for x in listing()[0] if x[0] == 4] == ["Social", "Raid nights"]
    wait_tree(mp, lambda t: [c for c, _, _ in t if c][:1] == ["SOCIAL"], "member saw the categories move")
    dp.evaluate(f"() => {{ state.voiceStates.delete({json.dumps(member['user']['id'])}); renderChannels(); }}")

    # ---- Collapse (remembered on the server) ----
    dp.click(item("signups") + " .channel")
    dp.click("#channel-tree .category-head:has-text('RAID NIGHTS')")
    dp.wait_for_selector("#channel-tree .ch-category.collapsed:has(.category-head:has-text('RAID NIGHTS'))")
    # The open channel still shows; the others fold away.
    assert dp.locator(item("signups")).count() == 1 and dp.locator(item("strats")).count() == 0
    dp.screenshot(path=f"{shots}/95-desktop-collapsed.png")
    end = time.time() + 10
    while time.time() < end and not next(c for c in listing()[1] if c["id"] == raids["id"])["collapsed"]:
        time.sleep(0.2)
    dp.reload()
    dp.wait_for_selector("#channel-tree .ch-category.collapsed:has(.category-head:has-text('RAID NIGHTS'))", timeout=15000)
    assert not next(c for c in listing(TM)[1] if c["id"] == raids["id"])["collapsed"], "collapsing wasn't per person"
    dp.click("#channel-tree .category-head:has-text('RAID NIGHTS')")
    dp.wait_for_selector(item("strats"))

    # ---- Synced / Not synced on the Permissions tab ----
    dp.click(item("strats") + " .channel", button="right")
    dp.click(".menu button:has-text('Edit permissions')")
    dp.wait_for_selector("dialog.edit-channel .ec-sync-badge", timeout=10000)
    assert dp.inner_text("dialog.edit-channel .ec-sync-badge") == "Synced"
    dp.keyboard.press("Escape")
    dp.click(item("chat") + " .channel", button="right")
    dp.click(".menu button:has-text('Edit permissions')")
    dp.wait_for_selector("dialog.edit-channel .ec-sync-badge.off", timeout=10000)
    dp.wait_for_timeout(300)
    dp.screenshot(path=f"{shots}/95-desktop-not-synced.png")
    dp.click("dialog.edit-channel .ec-sync-btn")
    dp.wait_for_selector("dialog.edit-channel .ec-sync-badge.on", timeout=10000)
    dp.keyboard.press("Escape")

    # ---- Members: a private category stays hidden; no moving ----
    officers = api("POST", f"/api/guilds/{G}/channels", {"name": "Officers", "kind": 4}, T)
    api("PUT", f"/api/channels/{officers['id']}/permissions/{G}", {"kind": "role", "allow": "0", "deny": "1"}, T)
    api("POST", f"/api/guilds/{G}/channels", {"name": "officer-talk", "parent_id": officers["id"], "sync_permissions": True}, T)
    dp.wait_for_selector("#channel-tree .category-head:has-text('OFFICERS')", timeout=10000)
    mp.reload()
    mp.wait_for_selector("#channel-tree .category-head:has-text('SOCIAL')", timeout=15000)
    mp.wait_for_timeout(500)
    assert mp.locator("#channel-tree .category-head:has-text('OFFICERS')").count() == 0, "a member saw a private category"
    assert mp.locator("#channel-tree [draggable='true']").count() == 0
    mp.click(item("strats") + " .channel", button="right")
    menu = mp.inner_text(".menu")
    assert "Move up" not in menu and "Move to category" not in menu, menu
    mp.keyboard.press("Escape")
    mp.click("#channel-tree .category-head:has-text('SOCIAL')", button="right")
    menu = mp.inner_text(".menu")
    assert "Collapse category" in menu and "Rename" not in menu and "Delete" not in menu, menu
    mp.keyboard.press("Escape")
    mp.screenshot(path=f"{shots}/95-desktop-member-view.png")
    mp.close()

    # "View outhouse as role" hides the private category too.
    dp.evaluate("startRolePreview([])")
    dp.wait_for_selector("#preview-banner")
    dp.wait_for_timeout(300)
    assert dp.locator("#channel-tree .category-head:has-text('OFFICERS')").count() == 0
    assert dp.locator("#channel-tree [draggable='true']").count() == 0, "dragging while previewing"
    dp.evaluate("exitRolePreview()")
    dp.wait_for_selector("#channel-tree .category-head:has-text('OFFICERS')", timeout=10000)
    dp.close()

    # ---- Phone: menus instead of dragging ----
    pp = page(T, "categories-phone", PHONE, channel=strats["id"])
    if pp.locator("#nav-toggle").is_visible():
        pp.click("#nav-toggle")
        pp.wait_for_timeout(400)
    assert pp.locator("#channel-tree [draggable='true']").count() == 0, "dragging on a phone"
    pp.screenshot(path=f"{shots}/95-phone-categories.png")
    before = [x for x in tree_order(pp) if x[0] == "RAID NIGHTS"]
    assert before[0][2] == "strats", before
    pp.click(item("strats") + " .channel", button="right")
    pp.wait_for_selector(".menu button:has-text('Move down')")
    pp.screenshot(path=f"{shots}/95-phone-channel-menu.png")
    pp.click(".menu button:has-text('Move down')")
    wait_tree(pp, lambda t: [x[2] for x in t if x[0] == "RAID NIGHTS" and x[1] == "text"][:2] == ["signups", "strats"], "Move down")
    pp.click(item("strats") + " .channel", button="right")
    pp.click(".menu button:has-text('Move up')")
    wait_tree(pp, lambda t: [x[2] for x in t if x[0] == "RAID NIGHTS" and x[1] == "text"][:2] == ["strats", "signups"], "Move up")
    pp.click(item("signups") + " .channel", button="right")
    pp.click(".menu button:has-text('Move to category')")
    pp.wait_for_selector(".menu >> nth=1")
    pp.screenshot(path=f"{shots}/95-phone-move-to-category.png")
    pp.click(".menu >> nth=1 >> button:has-text('SOCIAL')")
    wait_tree(pp, lambda t: ["SOCIAL", "text", "signups"] in t, "Move to category")
    if pp.locator(".cat-sync-offer").count():
        pp.click(".cat-sync-offer button.ghost")
    pp.click(item("signups") + " .channel", button="right")
    pp.click(".menu button:has-text('Move to category')")
    pp.click(".menu >> nth=1 >> button:has-text('No category')")
    wait_tree(pp, lambda t: ["", "text", "signups"] in t, "back to no category")
    # Categories move up and down from their menu.
    pp.click("#channel-tree .category-head:has-text('RAID NIGHTS')", button="right")
    pp.click(".menu button:has-text('Move up')")
    wait_tree(pp, lambda t: [c for c, _, _ in t if c][:1] == ["RAID NIGHTS"], "category Move up")
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/95-phone-after-moves.png")
    pp.click(item("chat") + " .channel", button="right")
    pp.click(".menu button:has-text('Edit permissions')")
    pp.wait_for_selector("dialog.edit-channel .ec-sync-badge", timeout=10000)
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/95-phone-sync-badge.png")
    pp.keyboard.press("Escape")
    pp.wait_for_selector("dialog.edit-channel", state="detached")
    shape = listing()[0]
    assert [x for x in shape if x[0] == 4][0][1] == "Raid nights", shape
    pp.close()

    # ---- Deleting a category keeps its channels ----
    api("DELETE", f"/api/channels/{social['id']}", None, T)
    after = listing()[0]
    assert (0, "front-porch", None) in after and all(x[1] != "Social" for x in after), after

    # The audit log has one entry per drag.
    moves = api("GET", f"/api/guilds/{G}/audit-log?action=channels_move", None, T)["entries"]
    assert len(moves) >= 8, len(moves)


if __name__ == "__main__":
    # Local run: BASE=http://127.0.0.1:8291 SHOTS=/tmp/shots python3 ci/ui_categories.py
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
        make = b.new_page
        b.new_page = lambda *a, **kw: make(*a, **{"bypass_csp": True, **kw})
        run(b, os.environ.get("BASE", "http://127.0.0.1:8080"), shots, watch)
        b.close()
    print("\n".join(errors) or "no errors")
    if errors:
        raise SystemExit(1)
