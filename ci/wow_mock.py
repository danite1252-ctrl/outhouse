"""A tiny stand-in for Wowhead and Battle.net, so CI can test /item, WoW
link cards and Battle.net linking without the internet.

    python3 ci/wow_mock.py [port]     (default 8099)

Wowhead:   /tooltip/item/<id>, /classic/search/suggestions-template?q=…,
           zone/NPC/object pages with `new Listview({...})` loot tables:
           trimmed copies of real pages in ci/wowhead/ (Molten Core with its
           Season of Discovery rows, Onyxia with drops only on her NPC page,
           Ragnaros, the Cache of the Firelord, Karazhan, Attumen, the Big Bad
           Wolf, the Chess event's chest), redirected to their full address like the real
           site; made-up /classic/npc=12397, /classic/object=181366 and
           /forever/zone=99001; other /forever pages answer 403 like
           wowhead.com does (2159 redirects); /classic/zone=3456 answers 503;
           /__down?on=1 makes every page 503), icons under /icons/ (names
           starting "missing" 404). Pages need a User-Agent that says Outhouse.
Battle.net /oauth/token (authorization code and client credentials),
           /oauth/userinfo, /api/profile/user/wow?namespace=…,
           WoW: Forever's Armory (profile-forever-{region}):
           /api/profile/wow/character/{name-surname}[/equipment|/character-media],
           portraits under /render/…, items in static-forever-{region}
           (/api/data/wow/item/{id}, /api/data/wow/media/item/{id},
           /api/data/wow/search/item), /api/data/wow/ruleset/index.
Test hooks: item 777 and the search "slow" take 8 s (longer than Outhouse
waits), the search "broken" answers 500, the character "busy-bee" answers
429 (Retry-After: 2), EU doesn't know Mockadin yet (404), Breezy Cloudsong
gains a level each time she's looked at, and /__hits counts requests.
"""

import json
import os
import sys
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
HITS = Counter()

ITEMS = {
    19019: {
        "name": "Thunderfury, Blessed Blade of the Windseeker",
        "quality": 5,
        "icon": "inv_sword_39",
        "tooltip": '<table><tr><td><!--nstart--><b class="q5">Thunderfury, Blessed Blade of the Windseeker</b><!--nend-->'
                   '<br>Binds when picked up<br>Unique<table width="100%"><tr><td>One-Hand</td><th>Sword</th></tr></table>'
                   '<table width="100%"><tr><td><span>44 - 115 Damage</span></td><th>Speed 1.90</th></tr></table>'
                   '(41.6 damage per second)<br>+5 Agility<br>+8 Stamina<br>+8 Fire Resistance<br>+9 Nature Resistance<br>'
                   'Item Level <!--ilvl-->80<br>Requires Level 60</td></tr></table>',
    },
    16922: {
        "name": "Leggings of Transcendence",
        "quality": 4,
        "icon": "inv_pants_08",
        "tooltip": '<table><tr><td><b class="q4">Leggings of Transcendence</b><br>Binds when picked up'
                   '<table width="100%"><tr><td>Legs</td><th>Cloth</th></tr></table>117 Armor<br>+16 Stamina<br>'
                   '+21 Intellect<br>Item Level <!--ilvl-->76<br>Requires Level 60<br>'
                   '<span class="q">Classes: Priest</span></td></tr></table>',
    },
}

ITEMS[17076] = {
    "name": "Bonereaver's Edge",
    "quality": 4,
    "icon": "inv_sword_12",
    "tooltip": '<table><tr><td><b class="q4">Bonereaver\'s Edge</b><br><!--bo-->Binds when picked up'
               '<table width="100%"><tr><td>Two-Hand</td><th>Sword</th></tr></table>'
               '<table width="100%"><tr><td><span>206 - 310 Damage</span></td><th>Speed 3.40</th></tr></table>'
               '(75.9 damage per second)<br>+16 Stamina<br>+16 Strength<br>Durability 120 / 120<br>'
               'Item Level <!--ilvl-->77<br>Requires Level 60<br>'
               '<span class="q2">Chance on hit: Your attacks ignore 700 of your enemies&#39; armor for 10 sec.</span>'
               '</td></tr></table>',
}

SEARCH = {
    "thunderfury": [
        {"type": 3, "id": 18564, "name": "Bindings of the Windseeker", "quality": 5},
        {"type": 3, "id": 19019, "name": "Thunderfury, Blessed Blade of the Windseeker", "quality": 5},
        {"type": 1, "id": 12056, "name": "Thunderfury NPC"},
    ],
    "leggings of transcendence": [{"type": 3, "id": "16922", "name": "Leggings of Transcendence", "typeName": "Item"}],
}

# Classic Era (profile-classic1x-us): name and realm.
CHARACTERS = {
    "wow_accounts": [{"id": 1, "characters": [
        {"name": "Twinky", "level": 19, "realm": {"name": "Nightslayer", "slug": "nightslayer"}, "playable_class": {"name": "Rogue"}},
        {"name": "Deathy", "level": 60, "realm": {"name": "Nightslayer", "slug": "nightslayer"}, "playable_class": {"name": "Death Knight"}},
    ]}],
}

# WoW: Forever (profile-forever-us): first and last names, no realm.
def forever_list():
    n = HITS["forever-list"]
    return {"wow_accounts": [{"id": 1, "characters": [
        {"id": 1001, "name": "Mockadin", "surname": "Lightforge", "level": 60,
         "playable_class": {"id": 2, "name": "Paladin"}, "playable_race": {"id": 1, "name": "Human"},
         "faction": {"type": "ALLIANCE", "name": "Alliance"}},
        # A level more each time the list is read (re-import updates her).
        {"id": 1002, "name": "Breezy", "surname": "Cloudsong", "level": 36 + n,
         "playable_class": {"id": 7, "name": "Shaman"}, "playable_race": {"name": "Skyborne"},
         "faction": {"type": "HORDE", "name": "Horde"}, "ruleset": {"id": 15}},
        {"id": 1003, "name": "Deathy", "surname": "Grimm", "level": 60,
         "playable_class": {"name": "Death Knight"}, "playable_race": {"id": 5, "name": "Undead"},
         "faction": {"type": "HORDE"}},
    ]}]}


# The Armory (public profiles, app token).
def armory_profile(slug, region, count=False):
    if slug == "mockadin-lightforge" and region == "us":
        return {"id": 1001, "name": "Mockadin", "surname": "Lightforge", "level": 60,
                "playable_class": {"id": 2, "name": "Paladin"}, "playable_race": {"id": 1, "name": "Human"},
                "faction": {"type": "ALLIANCE", "name": "Alliance"}, "ruleset": {"id": 2, "name": "Normal"},
                "guild": {"id": 77, "name": "Forever Flush"}, "equipped_item_level": 63, "average_item_level": 64}
    if slug == "breezy-cloudsong" and region == "us":
        if count:
            HITS["breezy"] += 1
        return {"id": 1002, "name": "Breezy", "surname": "Cloudsong", "level": 36 + HITS["breezy"],
                "playable_class": {"id": 7, "name": "Shaman"}, "playable_race": {"id": 52, "name": "Skyborne"},
                "faction": {"type": "HORDE", "name": "Horde"}, "ruleset": {"id": 15},
                "guild": {"id": 78, "name": "Throne Room"}}
    if slug == "lightwell-dawnstar":
        return {"id": 1004, "name": "Lightwell", "surname": "Dawnstar", "level": 60,
                "playable_class": {"id": 5, "name": "Priest"}, "playable_race": {"id": 1, "name": "Human"},
                "faction": {"type": "ALLIANCE"}, "ruleset": {"id": 1, "name": "PvP"},
                "guild": {"name": "Forever Flush"}, "equipped_item_level": 66}
    if slug == "sunny-skyward":
        return {"id": 1005, "name": "Sunny", "surname": "Skyward", "level": 52,
                "playable_class": {"id": 8, "name": "Mage"}, "playable_race": {"id": 52, "name": "Skyborne"},
                "faction": {"type": "ALLIANCE", "name": "Alliance"}, "ruleset": {"id": 3, "name": "Roleplay"},
                "guild": {"name": "Cloud Nine"}, "equipped_item_level": 48}
    return None


EQUIPMENT = {"equipped_items": [
    {"slot": {"type": "HEAD"}, "level": {"value": 40}},
    {"slot": {"type": "SHIRT"}, "level": {"value": 1}},
    {"slot": {"type": "CHEST"}, "level": {"value": 44}},
    {"slot": {"type": "TABARD"}, "level": {"value": 1}},
]}


def png(rgb=(255, 140, 60), size=24):
    """A small square portrait (solid colour with a darker frame)."""
    import struct
    import zlib
    rows = b""
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            edge = x < 2 or y < 2 or x >= size - 2 or y >= size - 2
            row += bytes(c // 3 if edge else c for c in rgb)
        rows += bytes(row)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


PORTRAITS = {"1001": (240, 200, 90), "1002": (90, 180, 240), "1004": (250, 250, 250), "1005": (160, 120, 250)}

FOREVER_ITEMS = {
    990001: {"id": 990001, "name": "Skyborne Sash", "quality": {"type": "EPIC", "name": "Epic"}, "level": 62,
             "required_level": 60, "inventory_type": {"name": "Waist"}, "item_subclass": {"name": "Cloth"}},
}


# ---- Wowhead zone, NPC and object pages (loot lists) ----------------------
# Molten Core, Onyxia and Karazhan are trimmed copies of the real pages in
# ci/wowhead/ (Season of Discovery rows, modes, sourcemore, drop counts and
# all; the Rust unit tests read the same files). Like the real site, a page
# asked for without its name in the address is redirected to it
# (/classic/zone=2717 → /classic/zone=2717/molten-core).
WOWHEAD_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wowhead")
FIXTURES = {
    ("classic", "zone", 2717): "molten-core",
    ("classic", "zone", 2159): "onyxias-lair",
    ("classic", "npc", 10184): "onyxia",
    ("classic", "npc", 11502): "ragnaros",
    ("classic", "object", 179703): "cache-of-the-firelord",
    ("tbc", "zone", 3457): "karazhan",
    ("tbc", "npc", 16152): "attumen-the-huntsman",
    ("tbc", "npc", 17521): "the-big-bad-wolf",
    ("tbc", "object", 185119): "dust-covered-chest",
}

def fixture(branch, kind, ident):
    with open(os.path.join(WOWHEAD_DIR, f"{branch}-{kind}-{ident}.html"), encoding="utf-8") as f:
        return f.read()

# Made-up pages in the same shape, for imports and a WoW: Forever raid.
# Rows: (id, name, quality, icon, classs, subclass, slot, reqclass, source)
# where source is (name, type 1 NPC / 2 object, id) or None (no sourcemore).
SPIRE_ITEMS = [
    (990001, "Skyborne Sash", 4, "inv_belt_13", 4, 1, 6, 0, ("Stormcaller Vael", 1, 990101)),
    (990002, "Gale-Touched Pendant", 4, "inv_jewelry_necklace_17", 4, 0, 2, 0, ("Stormcaller Vael", 1, 990101)),
    (990003, "Thunderhead Greataxe", 5, "inv_axe_43", 2, 1, 17, 0, ("Stormcaller Vael", 1, 990101)),
]
KAZZAK_ITEMS = [
    (17111, "Blazefury Medallion", 4, "inv_jewelry_amulet_05", 4, 0, 2, 0, None),
    (17112, "Empyrean Demolisher", 4, "inv_hammer_13", 2, 4, 13, 0, None),
    (18546, "Infernal Headcage", 4, "inv_helmet_58", 4, 4, 1, 0, None),
    (17113, "Amberseal Keeper", 4, "inv_staff_21", 2, 10, 17, 0, None),
]
CHEST_ITEMS = [
    (22691, "Corrupted Ashbringer", 4, "inv_sword_2h_ashbringercorrupt", 2, 8, 17, 0, None),
    (22809, "Maul of the Redeemed Crusader", 4, "inv_mace_25", 2, 5, 17, 0, None),
    (22811, "Soulstring", 4, "inv_weapon_bow_17", 2, 2, 15, 0, None),
]

def _row(item, i, chance=None):
    iid, name, q, icon, cls, sub, slot, req, src = item
    d = {"classs": cls, "flags2": 24580, "id": iid, "level": 70, "name": name, "quality": q,
         "reqlevel": 60, "slot": slot, "source": [2], "subclass": sub}
    if req:
        d["reqclass"] = req
    if src:
        d["sourcemore"] = [{"bd": 1, "n": src[0], "t": src[1], "ti": src[2]}]
    if chance:
        d["modes"] = {"mode": [0], "0": {"count": chance[0], "outof": chance[1]}}
        d["count"], d["outof"] = chance
    return json.dumps(d)

def _gatherer(items):
    data = {str(i[0]): {"name_enus": i[1], "quality": i[2], "icon": i[3], "jsonequip": {"slotbak": i[6]}} for i in items}
    return f"WH.Gatherer.addData(3, 4, {json.dumps(data)});"

def wowhead_page(title, kind, items=None, bosses=None, drops_id="drops", chances=False):
    rows = ",".join(_row(it, i, (12 + i, 100) if chances else None) for i, it in enumerate(items or []))
    npcs = ""
    if bosses:
        npc_rows = [{"boss": 1, "classification": 3, "id": i, "name": n, "react": [-1, -1]} for i, n in bosses]
        npcs = f"new Listview({{template: 'npc', id: 'npcs', name: WH.TERMS.npcs, tabs: tabsRelated, parent: 'lkljbjkb574', data: {json.dumps(npc_rows)}}});"
    drops = ""
    if items is not None:
        drops = ("new Listview({template: 'item', id: '" + drops_id + "', name: WH.TERMS." + drops_id + ", tabs: tabsRelated, parent: 'lkljbjkb574', "
                 "extraCols: [Listview.extraCols.count, Listview.extraCols.percent], sort:['-percent', 'name'], "
                 "computeDataFunc: Listview.funcBox.initLootTable, data:[" + rows + "]});")
    return (f"<!DOCTYPE html><html><head><title>{title} - {kind} - Classic World of Warcraft</title></head><body>"
            f"<h1 class=\"heading-size-1\">{title}</h1><div id=\"tabs-generic\"></div><script>//<![CDATA[\n"
            f"{_gatherer(items or [])}\nvar tabsRelated = new Tabs({{parent: WH.ge('tabs-generic'), trackable: 'Zone'}});\n"
            f"{npcs}\n{drops}\ntabsRelated.flush();\n//]]></script></body></html>")

GENERATED = {
    ("forever", "zone", 99001): lambda: wowhead_page("Skyborne Spire", "Zone", SPIRE_ITEMS, [(990101, "Stormcaller Vael")]),
    ("classic", "npc", 12397): lambda: wowhead_page("Lord Kazzak", "NPC", KAZZAK_ITEMS, chances=True),
    ("classic", "object", 181366): lambda: wowhead_page("Four Horsemen Chest", "Object", CHEST_ITEMS, drops_id="contains"),
}
SLUGS = {**FIXTURES, ("classic", "npc", 12397): "lord-kazzak", ("classic", "object", 181366): "four-horsemen-chest"}

DOWN = {"on": False}
BLOCKED = ("<HTML><HEAD><TITLE>ERROR: The request could not be satisfied</TITLE></HEAD><BODY><H1>403 ERROR</H1>"
           "<H2>The request could not be satisfied.</H2><HR noshade size=\"1px\">Request blocked.</BODY></HTML>")

def wowhead_pages(path):
    """(status, body or Location) for a Wowhead page, or None if it's not one."""
    parts = path.strip("/").split("/")
    if len(parts) in (2, 3) and "=" not in parts[0]:
        branch, target = parts[0], parts[1]
        slug = parts[2] if len(parts) == 3 else None
    else:
        return None
    if "=" not in target:
        return None
    kind, _, ident = target.partition("=")
    if kind not in ("zone", "npc", "object") or not ident.isdigit():
        return None
    ident = int(ident)
    if DOWN["on"]:
        return 503, "Service Unavailable"
    key = (branch, kind, ident)
    if branch == "forever":
        if key in GENERATED:
            return 200, GENERATED[key]()
        if ident == 2159:
            return 302, "/classic" + path[len("/forever"):]  # sent elsewhere: not on this branch
        # What wowhead.com answers for its Forever section (CloudFront).
        return 403, BLOCKED
    if branch == "classic" and kind == "zone" and ident == 3456:
        return 503, "Service Unavailable"  # Naxxramas: "Wowhead is down"
    if key not in FIXTURES and key not in GENERATED:
        return 404, "Not found"
    if slug is None:
        return 301, f"{path.rstrip('/')}/{SLUGS[key]}"
    return 200, fixture(*key) if key in FIXTURES else GENERATED[key]()

def icon_png(name):
    h = sum(ord(c) * (i + 1) for i, c in enumerate(name))
    return png(((h * 37) % 200 + 40, (h * 91) % 200 + 40, (h * 53) % 200 + 40), 36)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("wow-mock: " + fmt % args + "\n")

    def send(self, code, body, ctype="application/json"):
        data = (json.dumps(body) if not isinstance(body, (str, bytes)) else body)
        data = data.encode() if isinstance(data, str) else data
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urlparse(self.path)
        q = parse_qs(url.query)
        HITS[url.path] += 1
        if url.path == "/__hits":
            return self.send(200, dict(HITS))
        if url.path == "/__down":
            DOWN["on"] = (q.get("on") or ["0"])[0] == "1"
            return self.send(200, DOWN)
        page = wowhead_pages(url.path)
        if page is not None:
            code, body = page
            if code in (301, 302):
                self.send_response(code)
                self.send_header("Location", body)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if code == 200 and "Outhouse" not in self.headers.get("User-Agent", ""):
                return self.send(403, "Say who you are", "text/plain")
            return self.send(code, body, "text/html; charset=utf-8")
        if url.path.startswith("/tooltip/item/"):
            item_id = int(url.path.rsplit("/", 1)[1] or 0)
            if item_id == 777:
                time.sleep(8)
            item = ITEMS.get(item_id)
            return self.send(200, item) if item else self.send(404, {"error": "not found"})
        if url.path.endswith("/search/suggestions-template"):
            term = (q.get("q") or [""])[0].strip().lower()
            if term == "slow":
                time.sleep(8)
            if term == "broken":
                return self.send(500, "oops", "text/plain")
            return self.send(200, {"search": term, "results": SEARCH.get(term, [])})
        if url.path == "/oauth/userinfo":
            if self.headers.get("Authorization") != "Bearer mock-token":
                return self.send(401, {"error": "unauthorized"})
            return self.send(200, {"id": 42, "battletag": "Mockadin#1234"})
        ns = (q.get("namespace") or [""])[0]
        if url.path == "/api/profile/user/wow":
            if self.headers.get("Authorization") != "Bearer mock-token":
                return self.send(401, {"error": "unauthorized"})
            if ns == "profile-classic1x-us":
                return self.send(200, CHARACTERS)
            if ns == "profile-forever-us":
                HITS["forever-list"] += 1
                return self.send(200, forever_list())
            return self.send(404, {"code": 404, "detail": "Not Found"})
        if url.path.startswith("/api/profile/wow/character/") or url.path.startswith("/api/data/wow/"):
            if self.headers.get("Authorization") != "Bearer mock-app-token":
                return self.send(401, {"error": "unauthorized"})
        if url.path.startswith("/api/profile/wow/character/"):
            if not ns.startswith("profile-forever-"):
                return self.send(404, {"code": 404, "detail": "Not Found"})
            region = ns.rsplit("-", 1)[1]
            parts = url.path[len("/api/profile/wow/character/"):].split("/")
            slug, rest = parts[0], (parts[1] if len(parts) > 1 else "")
            if slug == "busy-bee":
                self.send_response(429)
                self.send_header("Retry-After", "2")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            prof = armory_profile(slug, region, count=(rest == ""))
            if not prof:
                return self.send(404, {"code": 404, "type": "BLZWEBAPI00000404", "detail": "Not Found"})
            if rest == "equipment":
                return self.send(200, EQUIPMENT)
            if rest == "character-media":
                host = self.headers.get("Host", "127.0.0.1:8099")
                pid = str(prof["id"])
                return self.send(200, {"assets": [
                    {"key": "avatar", "value": f"http://{host}/render/forever-{region}/avatar-{pid}.png"},
                    {"key": "main-raw", "value": f"http://{host}/render/forever-{region}/main-raw-{pid}.png"},
                ]})
            return self.send(200, prof)
        if url.path.startswith("/render/"):
            pid = url.path.rsplit("-", 1)[-1].split(".")[0]
            return self.send(200, png(PORTRAITS.get(pid, (200, 200, 200))), "image/png")
        if url.path == "/api/data/wow/ruleset/index":
            return self.send(200, {"rulesets": [{"id": 1, "name": "PvP"}, {"id": 2, "name": "Normal"},
                                                {"id": 3, "name": "Roleplay"}, {"id": 15, "name": "Roleplay PvP"}]})
        if url.path == "/api/data/wow/search/item":
            if ns != "static-forever-us":
                return self.send(404, {"code": 404})
            term = (q.get("name.en_US") or [""])[0].strip().lower()
            hits = [{"data": {"id": i["id"], "name": {"en_US": i["name"]}}} for i in FOREVER_ITEMS.values() if term and term in i["name"].lower()]
            return self.send(200, {"page": 1, "results": hits})
        if url.path.startswith("/api/data/wow/media/item/"):
            item_id = int(url.path.rsplit("/", 1)[1] or 0)
            if item_id not in FOREVER_ITEMS:
                return self.send(404, {"code": 404})
            host = self.headers.get("Host", "127.0.0.1:8099")
            return self.send(200, {"assets": [{"key": "icon", "value": f"http://{host}/icons/inv_belt_13.jpg"}]})
        if url.path.startswith("/api/data/wow/item/"):
            item_id = int(url.path.rsplit("/", 1)[1] or 0)
            if ns != "static-forever-us" or item_id not in FOREVER_ITEMS:
                return self.send(404, {"code": 404})
            return self.send(200, FOREVER_ITEMS[item_id])
        if url.path.startswith("/icons/"):
            name = url.path[len("/icons/"):].rsplit(".", 1)[0]
            if name.startswith("missing") or not name.replace("_", "").replace("-", "").isalnum():
                return self.send(404, "no icon", "text/plain")
            return self.send(200, icon_png(name), "image/png")
        self.send(404, {"error": "unknown path"})

    def do_POST(self):
        url = urlparse(self.path)
        HITS[url.path] += 1
        length = int(self.headers.get("Content-Length") or 0)
        form = parse_qs(self.rfile.read(length).decode())
        if url.path == "/oauth/token" and (form.get("grant_type") or [""])[0] == "client_credentials":
            if self.headers.get("Authorization", "") != "Basic bW9jay1pZDptb2NrLXNlY3JldA==":
                return self.send(401, {"error": "invalid_client"})
            HITS["app-token"] += 1
            return self.send(200, {"access_token": "mock-app-token", "token_type": "bearer", "expires_in": 86399})
        if url.path == "/oauth/token":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Basic ") or (form.get("code") or [""])[0] != "good-code" \
                    or not (form.get("redirect_uri") or [""])[0].endswith("/api/bnet/callback"):
                return self.send(400, {"error": "invalid_grant"})
            return self.send(200, {"access_token": "mock-token", "token_type": "bearer", "expires_in": 86399})
        self.send(404, {"error": "unknown path"})


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
