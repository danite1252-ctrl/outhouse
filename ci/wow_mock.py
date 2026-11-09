"""A tiny stand-in for Wowhead and Battle.net, so CI can test /item, WoW
link cards and Battle.net linking without the internet.

    python3 ci/wow_mock.py [port]     (default 8099)

Wowhead:   /tooltip/item/<id>, /classic/search/suggestions-template?q=…,
           zone/NPC/object pages with `new Listview({...})` loot tables
           (/classic/zone=2717 Molten Core with tricky JS, /classic/zone=2159
           Onyxia with drops only on /classic/npc=10184, /classic/npc=12397,
           /classic/object=181366, /tbc/zone=3457, /forever/zone=99001; other
           /forever pages 404 (2159 redirects); /classic/zone=3456 answers 503;
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
# Rows: (id, name, quality, icon, classs, subclass, slot, reqclass, source)
# where source is (name, type 1 NPC / 2 object, id) or None (no sourcemore).
MOLTEN_DESTROYER = ("Molten Destroyer", 1, 11659)
MC_BOSSES = [(12118, "Lucifron"), (11982, "Magmadar"), (12259, "Gehennas"), (12057, "Garr"),
             (12264, "Shazzrah"), (12056, "Baron Geddon"), (12098, "Sulfuron Harbinger"),
             (11988, "Golemagg the Incinerator"), (12018, "Majordomo Executus"), (11502, "Ragnaros")]
def _src(name):
    return next(((n, 1, i) for i, n in MC_BOSSES if n == name), None)
MC_ITEMS = [
    (16800, "Arcanist Boots", 4, "inv_boots_07", 4, 1, 8, 128, _src("Lucifron")),
    (16805, "Felheart Gloves", 4, "inv_gauntlets_19", 4, 1, 10, 256, _src("Lucifron")),
    (17109, "Choker of Enlightenment", 4, "inv_jewelry_necklace_10", 4, 0, 2, 0, _src("Lucifron")),
    (17069, "Striker's Mark", 4, "inv_weapon_bow_08", 2, 2, 15, 0, _src("Magmadar")),
    (17065, "Medallion of Steadfast Might", 4, "inv_jewelry_amulet_03", 4, 0, 2, 0, _src("Magmadar")),
    (16814, "Pants of Prophecy", 4, "inv_pants_08", 4, 1, 7, 16, _src("Magmadar")),
    (16843, "Earthfury Legguards", 4, "inv_pants_03", 4, 3, 7, 64, _src("Magmadar")),
    (16812, "Gloves of Prophecy", 4, "inv_gauntlets_14", 4, 1, 10, 16, _src("Gehennas")),
    (16826, "Nightslayer Gloves", 4, "inv_gauntlets_21", 4, 2, 10, 8, _src("Gehennas")),
    (18564, "Bindings of the Windseeker", 5, "inv_misc_bandage_17", 12, 0, 0, 0, _src("Garr")),
    (17105, "Aurastone Hammer", 4, "inv_hammer_05", 2, 4, 21, 0, _src("Garr")),
    (18829, "Deep Earth Spaulders", 4, "inv_shoulder_33", 4, 3, 3, 0, _src("Garr")),
    (16811, "Boots of Prophecy", 4, "inv_boots_07", 4, 1, 8, 16, _src("Shazzrah")),
    (18563, "Bindings of the Windseeker", 5, "inv_misc_bandage_17", 12, 0, 0, 0, _src("Baron Geddon")),
    (17110, "Seal of the Archmagus", 4, "inv_jewelry_ring_21", 4, 0, 11, 0, _src("Baron Geddon")),
    (17074, "Shadowstrike", 4, "inv_spear_08", 2, 6, 17, 0, _src("Sulfuron Harbinger")),
    (17077, "Crimson Shocker", 4, "inv_wand_10", 2, 19, 26, 0, _src("Sulfuron Harbinger")),
    (17103, "Azuresong Mageblade", 4, "inv_sword_39", 2, 7, 21, 0, _src("Golemagg the Incinerator")),
    (18842, "Staff of Dominance", 4, "inv_staff_13", 2, 10, 17, 0, _src("Golemagg the Incinerator")),
    (17072, "Blastershot Launcher", 4, "inv_weapon_rifle_10", 2, 3, 26, 0, _src("Golemagg the Incinerator")),
    (18646, "The Eye of Divinity", 4, "inv_misc_gem_pearl_04", 12, 0, 0, 16, ("Cache of the Firelord", 2, 179703)),
    (18703, "Ancient Petrified Leaf", 4, "inv_misc_herb_08", 12, 0, 0, 4, ("Cache of the Firelord", 2, 179703)),
    (17204, "Eye of Sulfuras", 5, "inv_misc_gem_pearl_05", 12, 0, 0, 0, _src("Ragnaros")),
    (18814, "Choker of the Fire Lord", 4, "inv_jewelry_necklace_17", 4, 0, 2, 0, _src("Ragnaros")),
    (17076, "Bonereaver's Edge", 4, "inv_sword_12", 2, 8, 17, 0, _src("Ragnaros")),
    (17106, "Malistar's Defender", 4, "inv_shield_17", 4, 6, 14, 0, _src("Ragnaros")),
    (16922, "Leggings of Transcendence", 4, "inv_pants_08", 4, 1, 7, 16, _src("Ragnaros")),
    (17063, "Band of Accuria", 4, "inv_jewelry_ring_15", 4, 0, 11, 0, _src("Ragnaros")),
    (19137, "Onslaught Girdle", 4, "inv_belt_13", 4, 4, 6, 0, _src("Ragnaros")),
    (18817, "Crown of Destruction", 4, "inv_helmet_14", 4, 3, 1, 0, _src("Ragnaros")),
    (17102, "Cloak of the Shrouded Mists", 4, "inv_misc_cape_07", 4, 1, 16, 0, _src("Ragnaros")),
    (17010, "Fiery Core", 3, "inv_misc_gem_fireruby", 7, 0, 0, 0, MOLTEN_DESTROYER),
    (17011, "Lava Core", 3, "inv_misc_gem_bloodstone_02", 7, 0, 0, 0, MOLTEN_DESTROYER),
    (99901, "Smouldering Ash", 0, "missing_ash", 15, 0, 0, 0, MOLTEN_DESTROYER),
]
ONY_ITEMS = [
    (18423, "Head of Onyxia", 4, "inv_misc_head_dragon_01", 12, 0, 0, 0, None),
    (17078, "Sapphiron Drape", 4, "inv_misc_cape_06", 4, 1, 16, 0, None),
    (18205, "Eskhandar's Collar", 4, "inv_jewelry_necklace_05", 4, 0, 2, 0, None),
    (17075, "Vis'kag the Bloodletter", 4, "inv_sword_1h_blacksmithing_02", 2, 7, 13, 0, None),
    (17068, "Deathbringer", 4, "inv_axe_09", 2, 0, 13, 0, None),
    (18813, "Ring of Binding", 4, "inv_jewelry_ring_13", 4, 0, 11, 0, None),
    (16921, "Halo of Transcendence", 4, "inv_crown_01", 4, 1, 1, 16, None),
    (17067, "Ancient Cornerstone Grimoire", 4, "inv_misc_book_07", 4, 0, 23, 0, None),
    (15410, "Scale of Onyxia", 3, "inv_misc_monsterscales_15", 7, 0, 0, 0, None),
]
KARA_ITEMS = [
    (30480, "Fiery Warhorse's Reins", 4, "ability_mount_dreadsteed", 15, 5, 0, 0, ("Attumen the Huntsman", 1, 15550)),
    (28785, "The Lightning Capacitor", 4, "spell_nature_lightning", 4, 0, 12, 0, ("Shade of Aran", 1, 16524)),
    (28749, "King's Defender", 4, "inv_sword_59", 2, 7, 13, 0, ("Chess Event", 2, 185119)),
    (28770, "Nathrezim Mindblade", 4, "inv_weapon_shortblade_42", 2, 15, 21, 0, ("Prince Malchezaar", 1, 15690)),
    (28773, "Gorehowl", 4, "inv_axe_43", 2, 1, 17, 0, ("Prince Malchezaar", 1, 15690)),
    (28762, "Adornment of Stolen Souls", 4, "inv_jewelry_necklace_ahnqiraj_02", 4, 0, 2, 0, ("Prince Malchezaar", 1, 15690)),
]
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

def _row(item, i, tricky=False, chance=None):
    iid, name, q, icon, cls, sub, slot, req, src = item
    d = {"classs": cls, "flags2": 8192, "id": iid, "level": 70, "name": name, "quality": q,
         "reqlevel": 60, "slot": slot, "source": [2], "subclass": sub}
    if req:
        d["reqclass"] = req
    if src:
        d["sourcemore"] = [{"n": src[0], "t": src[1], "ti": src[2], "z": 2717}]
    text = json.dumps(d)
    extra = f",count:{chance[0]},outof:{chance[1]}" if chance else ""
    if tricky and i % 5 == 1:
        # Old-style: quality digit in front of the name, no quality key.
        d.pop("quality")
        d["name"] = f"{7 - q}{name}"
        text = json.dumps(d)
    if tricky and i % 5 == 2:
        # Single quotes, unquoted keys, an escaped apostrophe.
        esc = name.replace("'", "\\'")
        text = (f"{{classs:{cls},id:{iid},name:'{esc}',quality:{q},slot:{slot},subclass:{sub}"
                + (f",reqclass:{req}" if req else "")
                + (f",sourcemore:[{{n:'{src[0].replace(chr(39), chr(92) + chr(39))}',t:{src[1]},ti:{src[2]}}}]" if src else "") + "}")
    return text[:-1] + extra + "}"

def _gatherer(items):
    data = {str(i[0]): {"name_enus": i[1], "quality": i[2], "icon": i[3].upper() if i[0] % 2 else i[3], "jsonequip": {"slotbak": i[6]}} for i in items}
    return f"WH.Gatherer.addData(3, 4, {json.dumps(data)});"

def wowhead_page(title, kind, items=None, bosses=None, drops_id="drops", tricky=False, chances=False):
    rows = ",\n".join(_row(it, i, tricky, (12 + i, 100) if chances else None) for i, it in enumerate(items or []))
    npcs = ""
    if bosses:
        npc_rows = [{"boss": 1, "classification": 3, "id": i, "name": n, "react": [-1, -1]} for i, n in bosses]
        npc_rows.insert(1, {"classification": 1, "id": 11659, "name": "Molten Destroyer"})
        npcs = f"new Listview({{template: 'npc', id: 'npcs', name: WH.TERMS.npcs, tabs: tabsRelated, parent: 'lv-npcs', data: {json.dumps(npc_rows)}}});"
    drops = ""
    if items is not None:
        drops = ("new Listview({template: 'item', id: '" + drops_id + "', name: WH.TERMS." + drops_id + ", tabs: tabsRelated, parent: 'lkljbjkb574', "
                 "extraCols: [Listview.extraCols.count, Listview.extraCols.percent], sort: ['-percent', 'name'], "
                 "computeDataFunc: Listview.funcBox.initLootTable, onAfterCreate: function(lv) { if (lv && lv.x) { return {a: '}'}; } }, "
                 "note: WH.sprintf(WH.TERMS.lootnote_format, '<a href=\"/x\">' + WH.TERMS.more + '</a>'), "
                 "data: [\n" + rows + (",\n" if tricky else "") + "]});")
    return (f"<!DOCTYPE html><html><head><title>{title} - {kind} - World of Warcraft Classic</title></head><body>"
            f"<h1 class=\"heading-size-1\">{title}</h1><div id=\"tabs-generic\"></div><script>//<![CDATA[\n"
            f"{_gatherer(items or [])}\nvar tabsRelated = new Tabs({{parent: WH.ge('tabs-generic'), trackable: 'Zone'}});\n"
            f"{npcs}\n{drops}\ntabsRelated.flush();\n//]]></script></body></html>")

DOWN = {"on": False}

def wowhead_pages(path):
    """(status, body) for a Wowhead page, or None if it's not one."""
    parts = path.strip("/").split("/")
    if len(parts) == 1:
        branch, target = "", parts[0]
    elif len(parts) >= 2 and "=" not in parts[0]:
        branch, target = parts[0], parts[1]
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
    if branch == "forever":
        if kind == "zone" and ident == 99001:
            return 200, wowhead_page("Skyborne Spire", "Zone", SPIRE_ITEMS, [(990101, "Stormcaller Vael")])
        if ident == 2159:
            return 302, ""  # Wowhead sends branches it lacks elsewhere
        return 404, "Not found"
    if branch == "classic":
        if kind == "zone" and ident == 2717:
            return 200, wowhead_page("Molten Core", "Zone", MC_ITEMS, MC_BOSSES, tricky=True)
        if kind == "zone" and ident == 2159:
            # No drops table on the zone page: the boss's page has them.
            return 200, wowhead_page("Onyxia&#39;s Lair", "Zone", None, [(10184, "Onyxia")])
        if kind == "npc" and ident == 10184:
            return 200, wowhead_page("Onyxia", "NPC", ONY_ITEMS, chances=True)
        if kind == "npc" and ident == 12397:
            return 200, wowhead_page("Lord Kazzak", "NPC", KAZZAK_ITEMS, chances=True)
        if kind == "object" and ident == 181366:
            return 200, wowhead_page("Four Horsemen Chest", "Object", CHEST_ITEMS, drops_id="contains")
        if kind == "zone" and ident == 3456:
            return 503, "Service Unavailable"  # Naxxramas: "Wowhead is down"
        return 404, "Not found"
    if branch == "tbc":
        if kind == "zone" and ident == 3457:
            return 200, wowhead_page("Karazhan", "Zone", KARA_ITEMS, [(15550, "Attumen the Huntsman"), (16524, "Shade of Aran"), (15690, "Prince Malchezaar")])
        return 404, "Not found"
    return 404, "Not found"

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
            if code == 302:
                self.send_response(302)
                self.send_header("Location", "/classic" + url.path[len("/forever"):])
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
