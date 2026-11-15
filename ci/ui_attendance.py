"""UI test for raid attendance from Method Raid Tools (web/mrt.js and the
By character tab in web/lootlog.js): importing the owner's real MRT export
on a raid card and a two-night grid in the Loot & attendance window (the
preview's comparison with sign-ups), the raid card's Attendance section,
the no-show hint raid leaders see (and members don't), attendance % on the
By character tab, and attendance per character on the full profile page.
Then hand marks in the same Attendance section (Edit attendance: marking
someone MRT missed, adding someone who didn't sign up, a raid with no
import), what members see, and the numbers counting both.
Desktop and phone screenshots (80-*, 85-*).

ci/ui_test.py calls run(); it can also run on its own against a local server
(see the bottom).
"""

import json
import os
import subprocess
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
SAMPLE = open(os.path.join(HERE, "mrt-attendance-sample.tsv"), encoding="utf-8").read()
NIGHTS = open(os.path.join(HERE, "mrt-attendance-two-nights.tsv"), encoding="utf-8").read()
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
                                                  "birth_date": "1990-01-01", "display_name": display}, xff=f"198.19.{int(run_id) % 200 + 30}.{n[0]}")

    lora, sham, kit, avi, plain, tent, benchy = (reg("mlora", "Lora"), reg("msham", "Sham"), reg("mkit", "Kit"), reg("mavi", "Avi"),
                                                 reg("mplain", "Plain"), reg("mtent", "Tent"), reg("mbench", "Benchy"))
    T = lora["token"]
    g = api("POST", "/api/guilds", {"name": "MRT Raiders"}, T)
    ch = api("POST", f"/api/guilds/{g['id']}/channels", {"name": "raids"}, T)
    code = api("POST", f"/api/guilds/{g['id']}/invites", None, T)["code"]
    for u in (sham, kit, avi, plain, tent, benchy):
        api("POST", f"/api/invites/{code}", None, u["token"])
    api("PUT", f"/api/guilds/{g['id']}/raid-settings", {"manager_role_ids": [], "realm_time_zone": "Australia/Sydney"}, T)
    chars = {
        "sham": api("POST", "/api/users/@me/characters", {"name": "Shambulàncè", "class": "Shaman", "level": 70, "game": "tbc", "is_main": True}, sham["token"]),
        "kit": api("POST", "/api/users/@me/characters", {"name": "Kitstab", "class": "Rogue", "level": 70, "game": "tbc", "is_main": True}, kit["token"]),
        "avi": api("POST", "/api/users/@me/characters", {"name": "Avizandum", "class": "Mage", "level": 70, "game": "tbc", "is_main": True}, avi["token"]),
    }
    soon = time.strftime("%Y-%m-%dT09:00:00Z", time.gmtime(time.time() + 4 * 86400))

    def raid(title):
        return api("POST", f"/api/channels/{ch['id']}/events", {"title": title, "starts_at": soon, "game": "tbc", "loot_instance": "bt",
                                                               "max_tanks": 2, "max_healers": 5, "max_dps": 18}, T)["event"]["id"]

    def signup(ev, who, status, cls, char=None, role="dps"):
        body = {"status": status, "role": role, "class": cls, "spec": "x"}
        if char:
            body["character_id"] = chars[char]["id"]
        api("PUT", f"/api/events/{ev}/signup", body, who["token"])

    R2, R3, R1 = raid("BT Wednesday"), raid("BT Thursday"), raid("BT Sunday")
    for ev in (R1, R2, R3):
        signup(ev, sham, "yes", "Shaman", "sham", "healer")
        signup(ev, kit, "yes", "Rogue", "kit")
        signup(ev, avi, "late" if ev == R2 else "yes", "Mage", "avi")
        signup(ev, plain, "yes", "Warrior", None, "tank")
    signup(R2, tent, "tentative", "Hunter")
    api("PUT", f"/api/events/{R2}/signup", {"status": "bench", "role": "dps", "class": "Warlock", "spec": "x", "user_id": benchy["user"]["id"]}, T)
    # The exports' nights, 8 pm Sydney.
    psql(f"UPDATE events SET starts_at = '2026-11-15T09:00:00Z' WHERE id = {R1}; "
         f"UPDATE events SET starts_at = '2026-11-11T09:00:00Z' WHERE id = {R2}; "
         f"UPDATE events SET starts_at = '2026-11-12T09:00:00Z' WHERE id = {R3}")

    def page(token, label, kw, wait_for=R1):
        pg = browser.new_page(**kw)
        watch(pg, label)
        pg.add_init_script(f"localStorage.setItem('outhouse_token', {json.dumps(token)}); localStorage.setItem('outhouse_guild', {json.dumps(g['id'])}); localStorage.setItem('outhouse_channel_{g['id']}', {json.dumps(ch['id'])});")
        pg.goto(base + "/")
        pg.wait_for_selector(f".event-card[data-event='{wait_for}']", timeout=15000)
        return pg

    def card(pg, ev_id):
        sel = f"#messages .event-card[data-event='{ev_id}']"
        pg.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", sel)
        pg.wait_for_timeout(300)
        return sel

    def group_count(pg, scope, group):
        return int(pg.inner_text(f"{scope} .mrt-group[data-group='{group}'] .mrt-n").strip())

    # ---- Phone first (nothing imported yet): the two-night grid's preview in the Loot & attendance window ----
    pp = page(T, "mrt-phone-preview", PHONE)
    pp.evaluate("openLootLog()")
    pp.wait_for_selector("dialog.loot-log-dialog button[data-act='mrt']", timeout=10000)
    pp.click("dialog.loot-log-dialog button[data-act='mrt']")
    pp.fill("dialog.mrt-dialog .gi-text", NIGHTS)
    pp.click("dialog.mrt-dialog button[data-act='preview']")
    pp.wait_for_selector(f"dialog.mrt-dialog .mrt-raid-compare[data-event='{R3}']", timeout=15000)
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/8010-phone-mrt-import-preview.png")
    pp.evaluate(f"document.querySelector(\"dialog.mrt-dialog .mrt-raid-compare[data-event='{R2}']\").scrollIntoView({{block: 'start'}})")
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/8011-phone-mrt-import-compare.png")
    pp.close()

    # ---- Desktop: raid card ⋯ → Import attendance (MRT)… → the owner's export ----
    lp = page(T, "mrt-desktop", DESKTOP)
    c1 = card(lp, R1)
    lp.click(f"{c1} .rc-menu")
    lp.click(".menu button:has-text('Import attendance (MRT)')")
    lp.wait_for_selector("dialog.mrt-dialog .gi-text")
    lp.fill("dialog.mrt-dialog .gi-text", SAMPLE)
    lp.click("dialog.mrt-dialog button[data-act='preview']")
    cmp1 = f"dialog.mrt-dialog .mrt-raid-compare[data-event='{R1}']"
    lp.wait_for_selector(cmp1, timeout=15000)
    assert lp.locator("dialog.mrt-dialog .mrt-col[data-col]").count() == 1
    lp.wait_for_selector("dialog.mrt-dialog .mrt-col[data-col='0'] .mrt-c-boss:has-text(\"High Warlord Naj'entus\") .mrt-enc")
    lp.wait_for_selector("dialog.mrt-dialog .mrt-col[data-col='0'] .mrt-diff:has-text('25-player')")
    assert lp.locator("dialog.mrt-dialog .mrt-grid tbody tr").count() == 22
    lp.wait_for_selector("dialog.mrt-dialog .mrt-grid tr[data-key='shambulance'] .mrt-name:has-text('Shambulàncè')")
    lp.wait_for_selector("dialog.mrt-dialog .mrt-grid tr[data-key='rumbet'] .mrt-name.unmatched .loot-warn")
    assert (group_count(lp, cmp1, "attended"), group_count(lp, cmp1, "missed"), group_count(lp, cmp1, "unsigned")) == (3, 1, 19)
    lp.wait_for_selector(f"{cmp1} .mrt-group[data-group='missed'] .mrt-name.noshow:has-text('Plain') .mrt-tag:has-text('no-show')")
    assert "1 no-show" in lp.inner_text(f"{cmp1} .mrt-raid-head")
    lp.wait_for_selector("dialog.mrt-dialog button[data-act='import']:has-text('Import 1 boss')")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/8000-desktop-mrt-import-preview.png")
    lp.click("dialog.mrt-dialog button[data-act='import']")
    lp.wait_for_selector("dialog.mrt-dialog .gi-done-head:has-text('Imported 1 boss')", timeout=15000)
    lp.wait_for_selector("dialog.mrt-dialog button[data-act='undo']")
    lp.click("dialog.mrt-dialog button:has-text('Done')")
    lp.wait_for_selector("dialog.mrt-dialog", state="detached")

    # ---- The raid card's Attendance section ----
    lp.wait_for_selector(f"{c1} details.ev-attendance", timeout=10000)
    lp.wait_for_function("s => document.querySelector(s)?.textContent.includes('22 attended')", arg=f"{c1} details.ev-attendance > summary", timeout=10000)
    summary = lp.inner_text(f"{c1} details.ev-attendance > summary")
    assert "22 attended · 1 no-show · 19 unsigned" in summary, summary
    lp.click(f"{c1} details.ev-attendance > summary")
    lp.wait_for_selector(f"{c1} details.ev-attendance .mrt-boss:has-text(\"Naj'entus\")", timeout=10000)
    title = lp.get_attribute(f"{c1} details.ev-attendance .mrt-group[data-group='attended'] .mrt-name:has-text('Kitstab')", "title")
    assert "✓ High Warlord Naj'entus 20:44" in title, title
    lp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/8002-desktop-raid-card-attendance.png")

    # ---- The Loot & attendance window: the two nights, by date and time ----
    lp.evaluate("openLootLog()")
    lp.wait_for_selector("dialog.loot-log-dialog h2:has-text('Loot & attendance')", timeout=10000)
    lp.click("dialog.loot-log-dialog button[data-act='mrt']")
    lp.fill("dialog.mrt-dialog .gi-text", NIGHTS)
    lp.click("dialog.mrt-dialog button[data-act='preview']")
    cmp2, cmp3 = (f"dialog.mrt-dialog .mrt-raid-compare[data-event='{r}']" for r in (R2, R3))
    lp.wait_for_selector(cmp3, timeout=15000)
    assert [lp.input_value(f"dialog.mrt-dialog .mrt-col[data-col='{i}'] select.mrt-raid") for i in range(3)] == [R2, R2, R3]
    lp.wait_for_selector(f"{cmp2} .mrt-group[data-group='partial'] .mrt-chip:has-text('Kitstab') .mrt-tag:has-text('left early')")
    lp.wait_for_selector(f"{cmp2} .mrt-group[data-group='partial'] .mrt-chip:has-text('Avizandum') .mrt-tag:has-text('joined late')")
    # Wednesday: Plain and Tent didn't come (no-shows, Tent tentative); Benchy was benched (doesn't count).
    lp.wait_for_selector(f"{cmp2} .mrt-group[data-group='missed'] .mrt-name.noshow:has-text('Tent') .mrt-tag:has-text('tentative')")
    lp.wait_for_selector(f"{cmp2} .mrt-group[data-group='missed'] .mrt-name:not(.noshow):has-text('Benchy') .mrt-tag.excused:has-text('bench')")
    assert "2 no-shows" in lp.inner_text(f"{cmp2} .mrt-raid-head"), lp.inner_text(f"{cmp2} .mrt-raid-head")
    # Thursday: Kit signed up and wasn't there.
    lp.wait_for_selector(f"{cmp3} .mrt-group[data-group='missed'] .mrt-name.noshow:has-text('Kitstab')")
    lp.wait_for_selector("dialog.mrt-dialog button[data-act='import']:has-text('Import 3 bosses')")
    lp.evaluate("document.querySelector('dialog.mrt-dialog .mrt-compares').scrollIntoView({block: 'start'})")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/8001-desktop-mrt-two-nights-compare.png")
    lp.click("dialog.mrt-dialog button[data-act='import']")
    lp.wait_for_selector("dialog.mrt-dialog .gi-done-head:has-text('Imported 3 bosses')", timeout=15000)
    lp.click("dialog.mrt-dialog button:has-text('Done')")

    # ---- By character: attendance % (all time), sortable ----
    lp.click("dialog.loot-log-dialog .seg-btn[data-tab='people']")
    lp.select_option("dialog.loot-log-dialog select.ll-range", "all")
    row = "dialog.loot-log-dialog .ll-people tr[data-character='{}']"
    lp.wait_for_selector(row.format("kitstab") + " .ll-pct", timeout=10000)

    def cells(key):
        return lp.eval_on_selector_all(row.format(key) + " td", "els => Object.fromEntries(els.map(e => [e.dataset.col, e.textContent.trim()]))")
    kc = cells("kitstab")
    assert kc["attendance"].startswith("67%") and "2 of 3" in kc["attendance"] and kc["noshows"] == "1", kc
    pc = cells("plain")
    assert pc["attendance"].startswith("0%") and pc["noshows"] == "3", pc
    assert cells("shambulance")["attendance"].startswith("100%")
    assert lp.locator(row.format("benchy")).count() == 0
    lp.wait_for_selector("dialog.loot-log-dialog .ll-range-label:has-text('all time · 3 raids with attendance')")
    # Lowest attendance first.
    lp.click("dialog.loot-log-dialog .ll-sort[data-psort='attendance']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-sort.on[data-psort='attendance'] .ll-arrow:has-text('▲')")
    first = lp.eval_on_selector("dialog.loot-log-dialog .ll-people tbody tr", "e => e.dataset.character")
    assert first == "plain", first
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/8003-desktop-by-character-attendance.png")
    lp.click("dialog.loot-log-dialog .seg-btn[data-tab='imports']")
    lp.wait_for_selector("dialog.loot-log-dialog .ll-import[data-att-import] button[data-act='undo']", timeout=10000)
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/8006-desktop-imports.png")
    lp.click("dialog.loot-log-dialog .raid-close")
    lp.close()

    # ---- Last week's raids, so the no-show hint (4 weeks) applies; next week's raid ----
    psql(f"UPDATE events SET starts_at = now() - interval '2 days' WHERE id = {R1}; "
         f"UPDATE events SET starts_at = now() - interval '6 days' WHERE id = {R2}; "
         f"UPDATE events SET starts_at = now() - interval '5 days' WHERE id = {R3}")
    R4 = raid("BT next week")
    signup(R4, plain, "yes", "Warrior", None, "tank")
    signup(R4, kit, "yes", "Rogue", "kit")
    signup(R4, sham, "yes", "Shaman", "sham", "healer")
    lp = page(T, "mrt-desktop-hint", DESKTOP, wait_for=R4)
    c4 = card(lp, R4)
    hint = f"{c4} .rc-entry[data-user='{plain['user']['id']}'] .rc-rely:not([hidden])"
    lp.wait_for_selector(hint, timeout=10000)
    assert lp.inner_text(hint).strip() == "3 no-shows", lp.inner_text(hint)
    assert "Only raid leaders see this" in lp.get_attribute(hint, "title")
    # Kit has one: no hint.
    assert lp.locator(f"{c4} .rc-entry[data-user='{kit['user']['id']}'] .rc-rely:not([hidden])").count() == 0
    lp.wait_for_timeout(200)
    lp.screenshot(path=f"{shots}/8005-desktop-reliability-hint.png")

    # ---- Kit's full profile: attendance per character ----
    lp.evaluate(f"openFullProfile({json.dumps(kit['user']['id'])})")
    lp.wait_for_selector("dialog.profile-page .fpa-char[data-character='kitstab'] .fpa-pct", timeout=10000)
    txt = lp.inner_text("dialog.profile-page .fpa-char[data-character='kitstab']")
    assert "67%" in txt and "attended 2 of 3" in txt and "1 no-show" in txt, txt
    lp.wait_for_selector("dialog.profile-page .fp-char .fp-char-att:has-text('67% attendance')")
    lp.evaluate("document.querySelector('dialog.profile-page #fp-attendance').scrollIntoView({block: 'center'})")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/8004-desktop-profile-attendance.png")
    lp.keyboard.press("Escape")
    lp.close()

    # ---- A member sees the Attendance section, but never the hint ----
    mp = page(kit["token"], "mrt-member", DESKTOP, wait_for=R4)
    c4 = card(mp, R4)
    mp.wait_for_timeout(800)
    assert mp.locator(".rc-rely").count() == 0
    c1 = card(mp, R1)
    mp.wait_for_selector(f"{c1} details.ev-attendance", timeout=10000)
    mp.close()

    # ---- Phone: the raid card's Attendance section, By character, the profile ----
    pp = page(T, "mrt-phone", PHONE)
    c1 = card(pp, R1)
    pp.click(f"{c1} details.ev-attendance > summary")
    pp.wait_for_selector(f"{c1} details.ev-attendance .mrt-group", timeout=10000)
    # Let the cards settle (the hint and sections load in), then scroll to the section.
    pp.wait_for_timeout(800)
    pp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance")
    pp.wait_for_timeout(400)
    pp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance")
    pp.wait_for_timeout(200)
    pp.screenshot(path=f"{shots}/8012-phone-raid-card-attendance.png")
    pp.evaluate("openLootLog({tab: 'people'})")
    pp.wait_for_selector("dialog.loot-log-dialog .ll-people tr[data-character='kitstab'] .ll-pct", timeout=10000)
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/8013-phone-by-character-attendance.png")
    pp.click("dialog.loot-log-dialog .raid-close")
    pp.evaluate(f"openFullProfile({json.dumps(kit['user']['id'])})")
    pp.wait_for_selector("dialog.profile-page .fpa-char", timeout=10000)
    pp.evaluate("document.querySelector('dialog.profile-page #fp-attendance').scrollIntoView({block: 'start'})")
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/8014-phone-profile-attendance.png")
    pp.close()

    # ---- One attendance: hand marks in the same 📋 Attendance section (Edit attendance) ----
    # Sunday: MRT missed Plain (joined after the last boss) and Tent came without signing up.
    lp = page(T, "att-merge-desktop", DESKTOP)
    c1 = card(lp, R1)
    if not lp.locator(f"{c1} details.ev-attendance[open]").count():
        lp.click(f"{c1} details.ev-attendance > summary")
    lp.click(f"{c1} details.ev-attendance button[data-act='edit-attendance']")
    ed = f"{c1} details.ev-attendance[open] .att-editor"
    plain_row = f"{ed} .att-row[data-user='{plain['user']['id']}']"
    lp.wait_for_selector(f"{plain_row} .att-outcome.o-no_show", timeout=10000)
    assert "MRT: not there" in lp.inner_text(plain_row), lp.inner_text(plain_row)
    kit_row = f"{ed} .att-row[data-user='{kit['user']['id']}']"
    assert "MRT: 1/1 boss" in lp.inner_text(kit_row) and "(from MRT)" in lp.inner_text(kit_row), lp.inner_text(kit_row)
    lp.click(f"{plain_row} .att-choice.present")
    lp.wait_for_selector(f"{plain_row}.marked.present .att-outcome.o-attended")
    lp.fill(f"{ed} .att-search", "Ten")
    lp.click(f"{ed} .att-add[data-user='{tent['user']['id']}']")
    lp.wait_for_selector(f"{ed} .att-row[data-user='{tent['user']['id']}'] .att-choice.present.on")
    lp.wait_for_selector(f"{ed} button[data-act='save-attendance']:has-text('Save attendance (2)')")
    lp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/85-desktop-edit-attendance.png")
    lp.click(f"{ed} button[data-act='save-attendance']")
    lp.wait_for_selector(f"{c1} details.ev-attendance .mrt-group[data-group='attended'] .mrt-name.manual:has-text('Plain') .mrt-tag.manual:has-text('present')", timeout=10000)
    lp.wait_for_selector(f"{c1} details.ev-attendance .mrt-group[data-group='unsigned'] .mrt-name.manual:has-text('Tent')")
    lp.wait_for_function("s => document.querySelector(s)?.textContent.includes('24 attended · 0 no-shows · 20 unsigned')", arg=f"{c1} details.ev-attendance > summary", timeout=10000)
    lp.wait_for_selector(f"{c1} details.ev-attendance .att-manual-note:has-text('2 people marked by hand')")
    lp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance")
    lp.wait_for_timeout(300)
    lp.screenshot(path=f"{shots}/85-desktop-attendance-merged.png")
    # A raid without an MRT import: Take attendance on the card opens the same section.
    R5 = raid("Karazhan pug")
    signup(R5, kit, "yes", "Rogue", "kit")
    signup(R5, sham, "yes", "Shaman", "sham", "healer")
    signup(R5, plain, "yes", "Warrior", None, "tank")
    psql(f"UPDATE events SET starts_at = now() - interval '1 hour' WHERE id = {R5}")
    lp.close()
    lp = page(T, "att-merge-manual", DESKTOP, wait_for=R5)
    c5 = card(lp, R5)
    lp.wait_for_selector(f"{c5} details.ev-attendance .ev-comp-sum:has-text('Not taken yet')", timeout=10000)
    lp.click(f"{c5} .ev-link.attendance:has-text('Take attendance')")
    ed5 = f"{c5} details.ev-attendance[open] .att-editor"
    lp.wait_for_selector(f"{ed5} .att-row", timeout=10000)
    lp.click(f"{ed5} .att-all")
    lp.click(f"{ed5} .att-row[data-user='{plain['user']['id']}'] .att-choice.excused")
    lp.click(f"{ed5} .att-row[data-user='{sham['user']['id']}'] .att-choice.absent")
    lp.wait_for_selector(f"{ed5} .att-row[data-user='{sham['user']['id']}'] .att-outcome.o-no_show")
    lp.click(f"{ed5} button[data-act='save-attendance']")
    lp.wait_for_function("s => document.querySelector(s)?.textContent.includes('1 attended · 1 no-show')", arg=f"{c5} details.ev-attendance > summary", timeout=10000)
    lp.wait_for_selector(f"{c5} details.ev-attendance .mrt-group[data-group='missed'] .mrt-name:not(.noshow):has-text('Plain') .mrt-tag:has-text('excused')")
    lp.wait_for_selector(f"{c5} .ev-link.attendance.done:has-text('Attendance ✓')")
    # By character counts both: Plain came to Sunday (by hand), missed Wednesday and Thursday, excused tonight.
    lp.evaluate("openLootLog({tab: 'people'})")
    prow = "dialog.loot-log-dialog .ll-people tr[data-character='plain'] td"
    lp.wait_for_selector(prow + "[data-col='attendance'] .ll-pct", timeout=10000)
    pc = lp.eval_on_selector_all(prow, "els => Object.fromEntries(els.map(e => [e.dataset.col, e.textContent.trim()]))")
    assert pc["attendance"].startswith("33%") and pc["noshows"] == "2", pc
    lp.click("dialog.loot-log-dialog .raid-close")
    lp.close()

    # A member sees the hand marks, but can't edit.
    mp = page(kit["token"], "att-merge-member", DESKTOP)
    c1 = card(mp, R1)
    mp.click(f"{c1} details.ev-attendance > summary")
    mp.wait_for_selector(f"{c1} details.ev-attendance .mrt-name.manual:has-text('Plain')", timeout=10000)
    assert mp.locator(f"{c1} details.ev-attendance [data-act='edit-attendance']").count() == 0
    assert mp.locator(f"{c1} .ev-link.attendance").count() == 0
    mp.close()

    # Phone: the merged section and its editor.
    pp = page(T, "att-merge-phone", PHONE)
    c1 = card(pp, R1)
    pp.click(f"{c1} details.ev-attendance > summary")
    pp.wait_for_selector(f"{c1} details.ev-attendance .att-manual-note", timeout=10000)
    # Let the cards settle (sections load in), then scroll to the section.
    pp.wait_for_timeout(1000)
    for _ in range(2):
        pp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} details.ev-attendance .mrt-group[data-group='attended']")
        pp.wait_for_timeout(400)
    pp.screenshot(path=f"{shots}/85-phone-attendance-merged.png")
    pp.click(f"{c1} details.ev-attendance button[data-act='edit-attendance']")
    pp.wait_for_selector(f"{c1} .att-editor .att-row[data-user='{plain['user']['id']}'].marked.present", timeout=10000)
    pp.evaluate("s => document.querySelector(s)?.scrollIntoView({block: 'start'})", f"{c1} .att-editor")
    pp.wait_for_timeout(300)
    pp.screenshot(path=f"{shots}/85-phone-edit-attendance.png")
    pp.click(f"{c1} .att-editor .att-save button.ghost")
    pp.wait_for_selector(f"{c1} details.ev-attendance .mrt-compare", timeout=10000)
    pp.close()


if __name__ == "__main__":
    # Local run: BASE=http://127.0.0.1:8282 DATABASE_URL=... SHOTS=/tmp/shots python3 ci/ui_attendance.py
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
