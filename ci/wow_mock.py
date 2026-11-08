"""A tiny stand-in for Wowhead and Battle.net, so CI can test /item, WoW
link cards and Battle.net linking without the internet.

    python3 ci/wow_mock.py [port]     (default 8099)

Wowhead:   /tooltip/item/<id>, /classic/search/suggestions-template?q=…
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
            return self.send(404, "no icon", "text/plain")
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
