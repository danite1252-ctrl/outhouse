"""A tiny stand-in for Wowhead and Battle.net, so CI can test /item, WoW
link cards and Battle.net linking without the internet.

    python3 ci/wow_mock.py [port]     (default 8099)

Wowhead:   /tooltip/item/<id>, /classic/search/suggestions-template?q=…
Battle.net /oauth/token, /oauth/userinfo, /api/profile/user/wow?namespace=…
Test hooks: item 777 and the search "slow" take 8 s (longer than Outhouse
waits), the search "broken" answers 500, and /__hits counts requests.
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

CHARACTERS = {
    "wow_accounts": [{"id": 1, "characters": [
        {"name": "Mockadin", "level": 60, "realm": {"name": "Forever", "slug": "forever"}, "playable_class": {"name": "Paladin"}},
        {"name": "Twinky", "level": 19, "realm": {"name": "Forever", "slug": "forever"}, "playable_class": {"name": "Rogue"}},
        {"name": "Deathy", "level": 60, "realm": {"name": "Forever", "slug": "forever"}, "playable_class": {"name": "Death Knight"}},
    ]}],
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
        if url.path == "/api/profile/user/wow":
            if self.headers.get("Authorization") != "Bearer mock-token":
                return self.send(401, {"error": "unauthorized"})
            if (q.get("namespace") or [""])[0] == "profile-classic1x-us":
                return self.send(200, CHARACTERS)
            return self.send(404, {"code": 404, "detail": "Not Found"})
        if url.path.startswith("/icons/"):
            return self.send(404, "no icon", "text/plain")
        self.send(404, {"error": "unknown path"})

    def do_POST(self):
        url = urlparse(self.path)
        HITS[url.path] += 1
        length = int(self.headers.get("Content-Length") or 0)
        form = parse_qs(self.rfile.read(length).decode())
        if url.path == "/oauth/token":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Basic ") or (form.get("code") or [""])[0] != "good-code" \
                    or not (form.get("redirect_uri") or [""])[0].endswith("/api/bnet/callback"):
                return self.send(400, {"error": "invalid_grant"})
            return self.send(200, {"access_token": "mock-token", "token_type": "bearer", "expires_in": 86399})
        self.send(404, {"error": "unknown path"})


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
