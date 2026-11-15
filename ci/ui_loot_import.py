"""UI test for loot history from Gargul (web/lootlog.js): importing the
owner's real export on a raid card and on the Loot page, the raid card's
Loot section, ✓ Won soft reserves (and the lock in the picker), the Loot
page's filters and per-character summary, and each character's loot on the
full profile page. Desktop and phone screenshots (70-*).

ci/ui_test.py calls run(); it can also run on its own against a local server
(see the bottom).
"""

import json
import os
import subprocess
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
SAMPLE = open(os.path.join(HERE, "gargul-sample.csv"), encoding="utf-8").read()
PHONE = {"viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True}
DESKTOP = {"viewport": {"width": 1280, "height": 900}}


def run(browser, base, shots, watch, db_url):
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

    def psql(sql):
        subprocess.run(["psql", db_url, "-qc", sql], check=True)

    run_id = str(int(time.time()))[-6:]
    n = [0]

    def reg(name, display):
        n[0] += 1
        return api("POST", "/api/auth/register", {"username": f"{name}{run_id}", "email": f"{name}{run_id}@example.test", "password": "password123",
                                                  "birth_date": "1990-01-01", "display_name": display}, xff=f"198.19.{int(run_id) % 200 + 20}.{n[0]}")

    lora, sham, dani, kit = reg("glora", "Lora"), reg("gsham", "Sham"), reg("gdani", "Dani"), reg("gkit", "Kit")
    T = lora["token"]
    g = api("POST", "/api/guilds", {"name": "Gargul Raiders"}, T)
    ch = api("POST", f"/api/guilds/{g['id']}/channels", {"name": "raids"}, T)
    code = api("POST", f"/api/guilds/{g['id']}/invites", None, T)["code"]
    for u in (sham, dani, kit):
        api("POST", f"/api/invites/{code}", None, u["token"])
    api("PUT", f"/api/guilds/{g['id']}/raid-settings", {"manager_role_ids": [], "realm_time_zone": "Australia/Sydney"}, T)
    c_sham = api("POST", "/api/users/@me/characters", {"name": "Shamièè", "class": "Shaman", "level": 70, "game": "tbc", "is_main": True}, sham["token"])
    api("POST", "/api/users/@me/characters", {"name": "Totemtoes", "class": "Druid", "level": 70, "game": "tbc"}, sham["token"])
    api("POST", "/api/users/@me/characters", {"name": "Danité", "class": "Priest", "level": 70, "game": "tbc"}, dani["token"])
    c_kit = api("POST", "/api/users/@me/characters", {"name": "Kitstab", "class": "Rogue", "level": 70, "game": "tbc"}, kit["token"])
    soon = time.strftime("%Y-%m-%dT09:00:00Z", time.gmtime(time.time() + 4 * 86400))
    r1 = api("POST", f"/api/channels/{ch['id']}/events", {"title": "Black Temple", "starts_at": soon, "game": "tbc", "loot_instance": "bt",
                                                         "soft_reserves": 2, "reserve_lock": "manual", "max_tanks": 2, "max_healers": 5, "max_dps": 18}, T)["event"]
    r2 = api("POST", f"/api/channels/{ch['id']}/events", {"title": "Serpentshrine", "starts_at": soon, "game": "tbc", "loot_instance": "ssc"}, T)["event"]
    for ev in (r1, r2):
        api("PUT", f"/api/events/{ev['id']}/signup", {"status": "yes", "role": "healer", "class": "Shaman", "spec": "Restoration", "character_id": c_sham["id"]}, sham["token"])
        api("PUT", f"/api/events/{ev['id']}/signup", {"status": "yes", "role": "dps", "class": "Rogue", "spec": "Combat", "character_id": c_kit["id"]}, kit["token"])
    api("PUT", f"/api/events/{r1['id']}/reserves", {"item_id": 32234}, sham["token"])
    api("PUT", f"/api/events/{r1['id']}/reserves", {"item_id": 32526}, sham["token"])
    api("PUT", f"/api/events/{r1['id']}/reserves", {"item_id": 32347}, kit["token"])
    # Back to the export's nights (8 pm Sydney), and where the items drop (the
    # outhouse's own Black Temple list).
    psql(f"UPDATE events SET starts_at = '2026-11-08T09:00:00Z' WHERE id = {r1['id']}; "
         f"UPDATE events SET starts_at = '2026-11-09T09:00:00Z' WHERE id = {r2['id']}; "
         "INSERT INTO loot_items (guild_id, instance, item_id, name, quality, boss) VALUES "
         f"({g['id']}, 'bt', 32240, 'Guise of the Tidal Lurker', 4, 'High Warlord Naj''entus'), "
         f"({g['id']}, 'bt', 32234, 'Fists of Mukoa', 4, 'High Warlord Naj''entus'), "
         f"({g['id']}, 'bt', 32261, 'Band of the Abyssal Lord', 4, 'High Warlord Naj''entus'), "
         f"({g['id']}, 'bt', 32254, 'The Brutalizer', 4, 'High Warlord Naj''entus'), "
         f"({g['id']}, 'bt', 32279, 'The Seeker''s Wristguards', 4, 'Supremus'), "
         f"({g['id']}, 'bt', 32268, 'Myrmidon''s Treads', 4, 'Supremus'), "
         f"({g['id']}, 'bt', 32347, 'Grips of Damnation', 4, 'Shade of Akama'), "
         f"({g['id']}, 'bt', 32526, 'Band of Devastation', 4, 'Gurtogg Bloodboil') ON CONFLICT DO NOTHING")
    R1, R2 = r1["id"], r2["id"]

    def page(token, label, kw):
        pg = browser.new_page(**kw)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(g['id'])}); localStorage.setItem('outhouse_channel_{g['id']}', {json.dumps(ch['id'])});")
        pg.goto(base + "/")
        pg.wait_for_selector(f".event-card[data-event='{R1}']", timeout=15000)
        return pg

    def card(pg, ev_id):
        sel = f"#messages .event-card[data-event='{ev_id}']"
        pg.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", sel)
        pg.wait_for_timeout(300)
        return sel

    # ---- Raid card: ⋯ → Import loot (Gargul)… → paste → preview ----
    late = SAMPLE.rstrip("\n") + "\n2026-11-12,Huktur,30000,0,LATE-1\n"
    lp = page(T, "gargul-desktop", DESKTOP)
    c1 = card(lp, R1)
    lp.click(f"{c1} .rc-menu")
    lp.click(".menu button:has-text('Import loot (Gargul)')")
    lp.wait_for_selector("dialog.gargul-dialog .gi-text")
    lp.fill("dialog.gargul-dialog .gi-text", late)
    lp.click("dialog.gargul-dialog button[data-act='preview']")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='12811834116478231330']", timeout=15000)
    assert lp.locator("dialog.gargul-dialog .gi-row[role='row']:not(.gi-headrow)").count() == 25
    # The 9th is the day after (late night): ticked. The 12th isn't this raid.
    assert lp.is_checked("dialog.gargul-dialog .gi-row[data-gargul='33618872553272877036'] input")
    assert not lp.is_checked("dialog.gargul-dialog .gi-row[data-gargul='LATE-1'] input")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='LATE-1'].out .gi-tag.out:has-text('not this raid')")
    row = "dialog.gargul-dialog .gi-row[data-gargul='12811834116478231330']"
    lp.wait_for_selector(f"{row} .loot-who.matched:has-text('Shamièè')")
    lp.wait_for_selector(f"{row} .loot-os")
    lp.wait_for_selector(f"{row} .item-name:has-text('Guise of the Tidal Lurker')")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='70184109116478231330'] .sr-count.won:has-text('SR')")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='77673436522395093700'].de .loot-de-who")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='10246397192049384983'] .loot-who.unmatched .loot-warn")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='10246397192049384983'] .item-name:has-text('Band of the Abyssal Lord')")
    # Not known anywhere: "Item 32527", still linked to Wowhead.
    href = lp.get_attribute("dialog.gargul-dialog .gi-row[data-gargul='35846349424649053890'] a.item-name", "href")
    assert href.endswith("/item=32527"), href
    # Only this raid's night: untick the 9th (it goes to Serpentshrine from the Loot page).
    for gid in lp.eval_on_selector_all("dialog.gargul-dialog .gi-row[data-gargul]", "els => els.filter(e => e.dataset.date === '2026-11-09').map(e => e.dataset.gargul)"):
        lp.uncheck(f"dialog.gargul-dialog .gi-row[data-gargul='{gid}'] input")
    lp.wait_for_selector("dialog.gargul-dialog button[data-act='import']:has-text('Import 14 items')")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/7000-desktop-gargul-import-preview.png")
    lp.click("dialog.gargul-dialog button[data-act='import']")
    lp.wait_for_selector("dialog.gargul-dialog .gi-done-head:has-text('Imported 14 items')", timeout=15000)
    lp.wait_for_selector("dialog.gargul-dialog button[data-act='undo']")
    lp.screenshot(path=f"{shots}/7001-desktop-gargul-imported.png")
    lp.click("dialog.gargul-dialog button:has-text('Done')")
    lp.wait_for_selector("dialog.gargul-dialog", state="detached")

    # ---- The raid card's Loot section (by boss, disenchanted folded away) and ✓ Won ----
    lp.wait_for_selector(f"{c1} details.ev-lootlog", timeout=10000)
    lp.wait_for_selector(f"{c1} .rc-entry .rc-sr.won:has-text('SR ✓')")
    lp.click(f"{c1} details.ev-lootlog > summary")
    lp.wait_for_selector(f"{c1} .rl-boss[data-boss=\"High Warlord Naj'entus\"] .rl-row", timeout=10000)
    lp.wait_for_selector(f"{c1} details.rl-de:not([open]) summary:has-text('Disenchanted (4)')")
    assert "10 won" in lp.inner_text(f"{c1} details.ev-lootlog > summary"), lp.inner_text(f"{c1} details.ev-lootlog > summary")
    lp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-lootlog")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/7002-desktop-raid-card-loot.png")
    # The soft reserves window: Sham's and Kit's reserves are ✓ Won.
    lp.click(f"{c1} .ev-link.sr")
    lp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='32234'] .sr-won-mark:has-text('Won')", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='32347'] .sr-person.won")
    assert not lp.query_selector("dialog.loot-dialog .sr-trow[data-item='32526'] .sr-won-mark"), "Band of Devastation wasn't won"
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/7003-desktop-sr-window-won.png")
    lp.click("dialog.loot-dialog .loot-tab[data-tab='won']")
    lp.wait_for_selector("dialog.loot-dialog .won-row.de .loot-de-who", timeout=10000)
    lp.wait_for_selector("dialog.loot-dialog .loot-won-import button[data-act='gargul']")
    lp.click("dialog.loot-dialog .raid-close")

    # ---- The Loot page: import the whole export (the 9th goes to Serpentshrine) ----
    lp.evaluate("openLootLog()")
    lp.wait_for_selector("dialog.loot-log-dialog .loot-log-table tbody tr", timeout=10000)
    lp.click("dialog.loot-log-dialog button[data-act='gargul']")
    lp.fill("dialog.gargul-dialog .gi-text", SAMPLE)
    lp.click("dialog.gargul-dialog button[data-act='preview']")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='33618872553272877036'] select.gi-raid", timeout=15000)
    assert lp.input_value("dialog.gargul-dialog .gi-row[data-gargul='33618872553272877036'] select.gi-raid") == R2
    lp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='33618872553272877036'] .loot-who.matched:has-text('Danité')")
    lp.wait_for_selector("dialog.gargul-dialog .gi-row.dup[data-gargul='12811834116478231330'] .gi-tag.dup")
    lp.wait_for_selector("dialog.gargul-dialog button[data-act='import']:has-text('Import 10 items')")
    lp.evaluate("document.querySelector(\"dialog.gargul-dialog .gi-row[data-gargul='21807218633949185442']\").scrollIntoView({block: 'center'})")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/7004-desktop-loot-page-import-preview.png")
    lp.click("dialog.gargul-dialog button[data-act='import']")
    lp.wait_for_selector("dialog.gargul-dialog .gi-done-head:has-text('Imported 10 items')", timeout=15000)
    lp.click("dialog.gargul-dialog button:has-text('Done')")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-count:has-text('20 items')", timeout=10000)
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/7005-desktop-loot-page.png")
    # Filters: a character (no accents needed), off spec only; then sorting.
    lp.fill("dialog.loot-log-dialog input[aria-label='Character']", "shamiee")
    lp.select_option("dialog.loot-log-dialog select[aria-label='Spec']", "os")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-count:has-text('3 items')", timeout=10000)
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/7006-desktop-loot-page-filtered.png")
    lp.fill("dialog.loot-log-dialog input[aria-label='Character']", "")
    lp.select_option("dialog.loot-log-dialog select[aria-label='Spec']", "")
    lp.check("dialog.loot-log-dialog input[aria-label='Include disenchanted']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-count:has-text('24 items')", timeout=10000)
    lp.click("dialog.loot-log-dialog .ll-sort[data-sort='item']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-sort.on[data-sort='item']", timeout=10000)
    lp.click("dialog.loot-log-dialog .seg-btn[data-tab='people']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-people tr[data-character='shamiee']", timeout=10000)
    cells = lp.eval_on_selector_all("dialog.loot-log-dialog .ll-people tr[data-character='shamiee'] td", "els => els.map(e => e.textContent.trim())")
    assert cells[1:5] == ["1", "3", "1", "4"], cells
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/7007-desktop-loot-by-character.png")
    lp.click("dialog.loot-log-dialog .seg-btn[data-tab='imports']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-import button:has-text('Undo')", timeout=10000)
    with lp.expect_download() as dl:
        lp.click("dialog.loot-log-dialog button[data-act='csv']")
    assert dl.value.suggested_filename.endswith("-loot.csv"), dl.value.suggested_filename
    lp.click("dialog.loot-log-dialog .raid-close")

    # ---- Sham's full profile: loot by character ----
    lp.evaluate(f"openFullProfile({json.dumps(sham['user']['id'])})")
    lp.wait_for_selector("dialog.profile-page .fpl-char[data-character='shamiee'] .fpl-row", timeout=10000)
    lp.wait_for_selector("dialog.profile-page .fpl-char[data-character='shamiee'] .fpl-count:has-text('this phase')")
    lp.wait_for_selector("dialog.profile-page .fp-char .fp-char-loot:has-text('this phase')")
    lp.evaluate("document.querySelector('dialog.profile-page #fp-wow').scrollIntoView({block: 'start'})")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/7008-desktop-profile-loot-history.png")
    lp.keyboard.press("Escape")

    # ---- Raid settings + the lock: Sham can't reserve Fists of Mukoa again in next week's BT ----
    nxt = api("POST", f"/api/channels/{ch['id']}/events", {"title": "Black Temple next week", "starts_at": soon, "game": "tbc", "loot_instance": "bt",
                                                          "soft_reserves": 2, "reserve_lock": "manual"}, T)["event"]
    api("PUT", f"/api/events/{nxt['id']}/signup", {"status": "yes", "role": "healer", "class": "Shaman", "spec": "Restoration", "character_id": c_sham["id"]}, sham["token"])
    lp.close()
    for label, kw in (("desktop", DESKTOP), ("phone", PHONE)):
        sp = page(sham["token"], f"gargul-sham-{label}", kw)
        sp.wait_for_selector(f".event-card[data-event='{nxt['id']}'] .ev-link.sr", timeout=15000)
        card(sp, nxt["id"])
        sp.click(f".event-card[data-event='{nxt['id']}'] .ev-link.sr")
        sp.fill("dialog.loot-dialog .loot-search", "mukoa")
        sp.wait_for_selector("dialog.loot-dialog .loot-row[data-item='32234'].locked-won .sr-pick.won-lock[disabled]:has-text('Won')", timeout=10000)
        sp.wait_for_timeout(250)
        sp.screenshot(path=f"{shots}/{7009 if label == 'desktop' else 7015}-{label}-sr-picker-won-lock.png")
        sp.close()

    # ---- Phone: import preview, raid card Loot, the Loot page, the profile ----
    pp = page(T, "gargul-phone", PHONE)
    c1 = card(pp, R1)
    pp.evaluate(f"openGargulImport(state.messages.find(m => m.event?.id === {json.dumps(R1)}).event)")
    pp.fill("dialog.gargul-dialog .gi-text", late)
    pp.click("dialog.gargul-dialog button[data-act='preview']")
    pp.wait_for_selector("dialog.gargul-dialog .gi-row[data-gargul='LATE-1']", timeout=15000)
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/7010-phone-gargul-import-preview.png")
    pp.click("dialog.gargul-dialog .raid-close")
    c1 = card(pp, R1)
    pp.click(f"{c1} details.ev-lootlog > summary")
    pp.wait_for_selector(f"{c1} .rl-row", timeout=10000)
    pp.wait_for_timeout(600)
    assert pp.eval_on_selector(f"{c1} details.ev-lootlog", "d => d.open")
    pp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-lootlog")
    pp.wait_for_timeout(400)
    print("phone loot card:", pp.evaluate("s => { const r = document.querySelector(s).getBoundingClientRect(); return [r.top, r.height]; }", f"{c1} details.ev-lootlog"))
    pp.screenshot(path=f"{shots}/7011-phone-raid-card-loot.png")
    pp.click(f"{c1} .ev-link.sr")
    pp.wait_for_selector("dialog.loot-dialog .sr-trow[data-item='32234'] .sr-won-mark", timeout=10000)
    pp.wait_for_timeout(200)
    pp.screenshot(path=f"{shots}/7012-phone-sr-window-won.png")
    pp.click("dialog.loot-dialog .raid-close")
    pp.evaluate("openLootLog()")
    pp.wait_for_selector("dialog.loot-log-dialog .loot-log-table tbody tr", timeout=10000)
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/7013-phone-loot-page.png")
    pp.click("dialog.loot-log-dialog .raid-close")
    pp.evaluate(f"openFullProfile({json.dumps(sham['user']['id'])})")
    pp.wait_for_selector("dialog.profile-page .fpl-char .fpl-row", timeout=10000)
    pp.evaluate("document.querySelector('dialog.profile-page #fp-loot').scrollIntoView({block: 'start'})")
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/7014-phone-profile-loot-history.png")
    pp.close()


if __name__ == "__main__":
    # Local run: BASE=http://127.0.0.1:8181 DATABASE_URL=... SHOTS=/tmp/shots python3 ci/ui_loot_import.py
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
        run(b, os.environ.get("BASE", "http://127.0.0.1:8080"), shots, watch, os.environ["DATABASE_URL"])
        b.close()
    print("\n".join(errors) or "no errors")
