//! Raid composition (roles, classes, armour, raid buffs, debuffs and
//! dispels) and soft-reserve loot (raid instances, a few famous items each,
//! reserves and the loot log).
//!
//! The tables here are plain data so they're easy to edit: add a row to
//! [`BUFFS`] for a buff, or to [`INSTANCES`] / [`SEED_ITEMS`] for a raid.
//! WoW: Forever may add or change raids, so raid leaders can also add items
//! to any raid's list from the app (by Wowhead link, ID or name) and pick
//! "Other" for a raid that isn't listed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{as_string, option_as_string};

// ---- Raid buffs ------------------------------------------------------------

/// Which buff list a game uses: Classic-era ("classic": WoW: Forever and
/// Classic Era) or Burning Crusade ("tbc").
pub fn era_of(game: &str) -> &'static str {
    if game == "tbc" {
        "tbc"
    } else {
        "classic"
    }
}

/// One raid buff, debuff, bit of utility or dispel, and who brings it.
pub struct Buff {
    pub key: &'static str,
    pub name: &'static str,
    /// "buff", "debuff", "utility", "dispel" or "note" (shown, not checked).
    pub kind: &'static str,
    /// "classic", "tbc" or "both".
    pub games: &'static str,
    /// (class, spec): spec "" means any spec of that class.
    pub providers: &'static [(&'static str, &'static str)],
    /// Words for the "what's missing" line when nobody brings it ("" = it
    /// isn't mentioned there).
    pub short: &'static str,
    pub note: &'static str,
}

const fn b(
    key: &'static str,
    name: &'static str,
    kind: &'static str,
    games: &'static str,
    providers: &'static [(&'static str, &'static str)],
    short: &'static str,
    note: &'static str,
) -> Buff {
    Buff {
        key,
        name,
        kind,
        games,
        providers,
        short,
        note,
    }
}

/// Every buff we check, in the order they're shown. Factions aren't
/// assumed: WoW: Forever raids can mix Alliance and Horde, so Paladins and
/// Shamans count together.
pub const BUFFS: &[Buff] = &[
    // Buffs
    b(
        "fortitude",
        "Power Word: Fortitude",
        "buff",
        "both",
        &[("Priest", "")],
        "Fortitude",
        "",
    ),
    b(
        "divine_spirit",
        "Divine Spirit",
        "buff",
        "both",
        &[("Priest", "Discipline")],
        "",
        "Discipline talent",
    ),
    b(
        "mark_of_the_wild",
        "Mark of the Wild",
        "buff",
        "both",
        &[("Druid", "")],
        "Mark of the Wild",
        "",
    ),
    b(
        "arcane_intellect",
        "Arcane Intellect",
        "buff",
        "both",
        &[("Mage", "")],
        "Arcane Intellect",
        "",
    ),
    b(
        "blessing_of_kings",
        "Blessing of Kings",
        "buff",
        "both",
        &[("Paladin", "")],
        "Blessings",
        "Protection talent; most raiding Paladins take it",
    ),
    b(
        "blessing_of_might",
        "Blessing of Might",
        "buff",
        "both",
        &[("Paladin", "")],
        "Blessings",
        "",
    ),
    b(
        "blessing_of_wisdom",
        "Blessing of Wisdom",
        "buff",
        "both",
        &[("Paladin", "")],
        "Blessings",
        "",
    ),
    b(
        "blessing_of_salvation",
        "Blessing of Salvation",
        "buff",
        "both",
        &[("Paladin", "")],
        "Blessings",
        "",
    ),
    b(
        "blessing_of_light",
        "Blessing of Light",
        "buff",
        "both",
        &[("Paladin", "")],
        "",
        "",
    ),
    b(
        "windfury_totem",
        "Windfury Totem",
        "buff",
        "both",
        &[("Shaman", "")],
        "Windfury",
        "Melee groups",
    ),
    b(
        "strength_of_earth",
        "Strength of Earth Totem",
        "buff",
        "both",
        &[("Shaman", "")],
        "",
        "",
    ),
    b(
        "grace_of_air",
        "Grace of Air Totem",
        "buff",
        "both",
        &[("Shaman", "")],
        "",
        "",
    ),
    b(
        "mana_spring",
        "Mana Spring Totem",
        "buff",
        "both",
        &[("Shaman", "")],
        "",
        "",
    ),
    b(
        "tranquil_air",
        "Tranquil Air Totem",
        "buff",
        "both",
        &[("Shaman", "")],
        "",
        "Less threat",
    ),
    b(
        "wrath_of_air",
        "Wrath of Air Totem",
        "buff",
        "tbc",
        &[("Shaman", "")],
        "",
        "",
    ),
    b(
        "totem_of_wrath",
        "Totem of Wrath",
        "buff",
        "tbc",
        &[("Shaman", "Elemental")],
        "",
        "Elemental talent",
    ),
    b(
        "unleashed_rage",
        "Unleashed Rage",
        "buff",
        "tbc",
        &[("Shaman", "Enhancement")],
        "",
        "Enhancement talent",
    ),
    b(
        "bloodlust",
        "Bloodlust / Heroism",
        "buff",
        "tbc",
        &[("Shaman", "")],
        "Bloodlust",
        "One per Shaman's group",
    ),
    b(
        "drums",
        "Drums of Battle / War",
        "note",
        "tbc",
        &[],
        "",
        "Leatherworkers bring these: not tracked here",
    ),
    b(
        "trueshot_aura",
        "Trueshot Aura",
        "buff",
        "both",
        &[("Hunter", "Marksmanship")],
        "",
        "Marksmanship talent",
    ),
    b(
        "ferocious_inspiration",
        "Ferocious Inspiration",
        "buff",
        "tbc",
        &[("Hunter", "Beast Mastery")],
        "",
        "Beast Mastery talent",
    ),
    b(
        "battle_shout",
        "Battle Shout",
        "buff",
        "both",
        &[("Warrior", "")],
        "Battle Shout",
        "",
    ),
    b(
        "blood_pact",
        "Blood Pact",
        "buff",
        "both",
        &[("Warlock", "")],
        "",
        "Imp",
    ),
    b(
        "moonkin_aura",
        "Moonkin Aura",
        "buff",
        "both",
        &[("Druid", "Balance")],
        "",
        "Balance talent",
    ),
    b(
        "leader_of_the_pack",
        "Leader of the Pack",
        "buff",
        "both",
        &[("Druid", "Feral (Cat)"), ("Druid", "Feral (Bear)")],
        "",
        "Feral talent",
    ),
    b(
        "improved_sanctity_aura",
        "Sanctity Aura",
        "buff",
        "tbc",
        &[("Paladin", "Retribution")],
        "",
        "Retribution talent",
    ),
    b(
        "vampiric_touch",
        "Vampiric Touch",
        "buff",
        "tbc",
        &[("Priest", "Shadow")],
        "",
        "Mana for the Shadow Priest's group",
    ),
    // Debuffs on the boss
    b(
        "sunder_armor",
        "Sunder Armor",
        "debuff",
        "both",
        &[("Warrior", "")],
        "Sunder Armor",
        "",
    ),
    b(
        "faerie_fire",
        "Faerie Fire",
        "debuff",
        "both",
        &[("Druid", "")],
        "",
        "",
    ),
    b(
        "curse_of_recklessness",
        "Curse of Recklessness",
        "debuff",
        "both",
        &[("Warlock", "")],
        "",
        "",
    ),
    b(
        "curse_of_the_elements",
        "Curse of the Elements",
        "debuff",
        "both",
        &[("Warlock", "")],
        "Curses",
        "",
    ),
    b(
        "curse_of_shadow",
        "Curse of Shadow",
        "debuff",
        "both",
        &[("Warlock", "")],
        "",
        "",
    ),
    b(
        "shadow_weaving",
        "Shadow Weaving",
        "debuff",
        "both",
        &[("Priest", "Shadow")],
        "",
        "Shadow talent",
    ),
    b(
        "misery",
        "Misery",
        "debuff",
        "tbc",
        &[("Priest", "Shadow")],
        "",
        "Shadow talent",
    ),
    b(
        "improved_scorch",
        "Improved Scorch",
        "debuff",
        "both",
        &[("Mage", "Fire")],
        "",
        "Fire talent",
    ),
    b(
        "winters_chill",
        "Winter's Chill",
        "debuff",
        "both",
        &[("Mage", "Frost")],
        "",
        "Frost talent",
    ),
    b(
        "hunters_mark",
        "Hunter's Mark",
        "debuff",
        "both",
        &[("Hunter", "")],
        "",
        "",
    ),
    b(
        "judgement_of_wisdom",
        "Judgement of Wisdom",
        "debuff",
        "both",
        &[("Paladin", "")],
        "",
        "",
    ),
    b(
        "blood_frenzy",
        "Blood Frenzy",
        "debuff",
        "tbc",
        &[("Warrior", "Arms")],
        "",
        "Arms talent",
    ),
    // Utility
    b(
        "innervate",
        "Innervate",
        "utility",
        "both",
        &[("Druid", "")],
        "",
        "",
    ),
    b(
        "battle_res",
        "Rebirth (battle res)",
        "utility",
        "both",
        &[("Druid", "")],
        "battle res",
        "",
    ),
    b(
        "soulstone",
        "Soulstone",
        "utility",
        "both",
        &[("Warlock", "")],
        "",
        "",
    ),
    b(
        "healthstone",
        "Healthstones",
        "utility",
        "both",
        &[("Warlock", "")],
        "Healthstones",
        "",
    ),
    b(
        "summon",
        "Ritual of Summoning",
        "utility",
        "both",
        &[("Warlock", "")],
        "",
        "",
    ),
    b(
        "tranq_shot",
        "Tranquilizing Shot",
        "utility",
        "classic",
        &[("Hunter", "")],
        "",
        "Removes enrage (Magmadar, Flamegor, Chromaggus, Huhuran)",
    ),
    b(
        "misdirection",
        "Misdirection",
        "utility",
        "tbc",
        &[("Hunter", "")],
        "",
        "",
    ),
    b(
        "mana_tide",
        "Mana Tide Totem",
        "utility",
        "both",
        &[("Shaman", "Restoration")],
        "",
        "Restoration talent",
    ),
    // Who can remove what
    b(
        "decurse",
        "Remove curses",
        "dispel",
        "both",
        &[("Mage", ""), ("Druid", "")],
        "decurse",
        "",
    ),
    b(
        "cure_poison",
        "Cure poison",
        "dispel",
        "both",
        &[("Druid", ""), ("Shaman", ""), ("Paladin", "")],
        "poison cure",
        "",
    ),
    b(
        "cure_disease",
        "Cure disease",
        "dispel",
        "both",
        &[("Priest", ""), ("Shaman", ""), ("Paladin", "")],
        "disease cure",
        "",
    ),
    b(
        "dispel_magic",
        "Dispel magic",
        "dispel",
        "both",
        &[("Priest", ""), ("Paladin", "")],
        "magic dispel",
        "",
    ),
];

/// The buffs that matter in a game ("forever", "era", "tbc" or "").
pub fn buffs_for(game: &str) -> impl Iterator<Item = &'static Buff> {
    let era = era_of(game);
    BUFFS
        .iter()
        .filter(move |b| b.games == "both" || b.games == era)
}

/// The armour a class wears at max level.
pub fn armour_of(class: &str) -> Option<&'static str> {
    Some(match class {
        "Mage" | "Priest" | "Warlock" => "cloth",
        "Druid" | "Rogue" => "leather",
        "Hunter" | "Shaman" => "mail",
        "Paladin" | "Warrior" => "plate",
        _ => return None,
    })
}

pub const ARMOUR: [&str; 4] = ["cloth", "leather", "mail", "plate"];

/// Someone counted in a raid's composition.
#[derive(Debug, Clone)]
pub struct CompMember {
    pub user_id: i64,
    pub name: String,
    pub class: String,
    pub spec: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleCount {
    /// "tank", "healer" or "dps".
    pub role: String,
    pub count: i32,
    pub max: i32,
    /// Slots still open (0 when full or over).
    pub missing: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassCount {
    /// One of the nine classes, or "" for people who didn't say.
    pub class: String,
    pub count: i32,
    /// Their names (character name, else outhouse name).
    pub people: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArmourCount {
    /// "cloth", "leather", "mail" or "plate".
    pub kind: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuffStatus {
    pub key: String,
    pub name: String,
    pub kind: String,
    /// Someone brings it (always false for notes).
    pub covered: bool,
    /// Who can bring it, e.g. ["Shadow Priest"] or ["Mage", "Druid"].
    pub sources: Vec<String>,
    /// The people in the raid who bring it.
    pub providers: Vec<String>,
    #[serde(default)]
    pub note: String,
}

/// `GET /api/events/{id}/composition`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Composition {
    #[serde(with = "as_string")]
    pub event_id: i64,
    /// "classic" or "tbc": which buffs were checked.
    pub era: String,
    /// Tentative and bench were counted too.
    pub include_extra: bool,
    /// How many people were counted.
    pub counted: i32,
    pub roles: Vec<RoleCount>,
    pub classes: Vec<ClassCount>,
    pub armour: Vec<ArmourCount>,
    pub buffs: Vec<BuffStatus>,
    /// "1 healer", "No Mage (Arcane Intellect)", "No decurse", …
    pub missing: Vec<String>,
    /// "Need: 1 healer · No Mage (Arcane Intellect) · No decurse" ("" when
    /// nothing's missing).
    pub missing_line: String,
}

fn provides(m: &CompMember, class: &str, spec: &str) -> bool {
    m.class == class && (spec.is_empty() || m.spec == spec)
}

fn source_label(class: &str, spec: &str) -> String {
    if spec.is_empty() {
        class.to_string()
    } else {
        // "Feral (Bear)" → "Feral Druid (Bear)" reads oddly; keep it simple.
        format!("{spec} {class}")
    }
}

fn role_words(role: &str, n: i32) -> String {
    match (role, n) {
        ("tank", 1) => "1 tank".into(),
        ("tank", n) => format!("{n} tanks"),
        ("healer", 1) => "1 healer".into(),
        ("healer", n) => format!("{n} healers"),
        (_, n) => format!("{n} DPS"),
    }
}

/// Works out a raid's composition from the people counted.
pub fn compose(
    event_id: i64,
    members: &[CompMember],
    game: &str,
    slots: [(&str, i32); 3],
    include_extra: bool,
) -> Composition {
    let roles: Vec<RoleCount> = slots
        .iter()
        .map(|&(role, max)| {
            let count = members.iter().filter(|m| m.role == role).count() as i32;
            RoleCount {
                role: role.into(),
                count,
                max,
                missing: (max - count).max(0),
            }
        })
        .collect();

    let mut classes: Vec<ClassCount> = Vec::new();
    for m in members {
        let class = crate::wow::class_name(&m.class).unwrap_or("").to_string();
        match classes.iter_mut().find(|c| c.class == class) {
            Some(c) => {
                c.count += 1;
                c.people.push(m.name.clone());
            }
            None => classes.push(ClassCount {
                class,
                count: 1,
                people: vec![m.name.clone()],
            }),
        }
    }
    classes.sort_by(|a, b| {
        (a.class.is_empty())
            .cmp(&b.class.is_empty())
            .then(b.count.cmp(&a.count))
            .then(a.class.cmp(&b.class))
    });

    let armour = ARMOUR
        .iter()
        .map(|&kind| ArmourCount {
            kind: kind.into(),
            count: members
                .iter()
                .filter(|m| armour_of(&m.class) == Some(kind))
                .count() as i32,
        })
        .collect();

    let buffs: Vec<BuffStatus> = buffs_for(game)
        .map(|b| {
            let providers: Vec<String> = members
                .iter()
                .filter(|m| b.providers.iter().any(|&(c, s)| provides(m, c, s)))
                .map(|m| m.name.clone())
                .collect();
            BuffStatus {
                key: b.key.into(),
                name: b.name.into(),
                kind: b.kind.into(),
                covered: b.kind != "note" && !providers.is_empty(),
                sources: b
                    .providers
                    .iter()
                    .map(|&(c, s)| source_label(c, s))
                    .collect(),
                providers,
                note: b.note.into(),
            }
        })
        .collect();

    // What's missing: open role slots, then classes nobody brings whose
    // key buffs nobody else covers, then dispels.
    let mut missing: Vec<String> = roles
        .iter()
        .filter(|r| r.missing > 0)
        .map(|r| role_words(&r.role, r.missing))
        .collect();
    let covered = |key: &str| buffs.iter().any(|s| s.key == key && s.covered);
    for class in crate::wow::CLASSES {
        if members.iter().any(|m| m.class == class) {
            continue;
        }
        let mut shorts: Vec<&str> = Vec::new();
        for b in buffs_for(game) {
            if b.short.is_empty()
                || b.kind == "dispel"
                || covered(b.key)
                || !b.providers.iter().any(|&(c, _)| c == class)
                || shorts.contains(&b.short)
            {
                continue;
            }
            shorts.push(b.short);
        }
        if !shorts.is_empty() {
            missing.push(format!("No {class} ({})", shorts.join(", ")));
        }
    }
    for b in buffs_for(game).filter(|b| b.kind == "dispel" && !b.short.is_empty()) {
        if !covered(b.key) {
            missing.push(format!("No {}", b.short));
        }
    }
    let missing_line = if missing.is_empty() {
        String::new()
    } else {
        format!("Need: {}", missing.join(" · "))
    };

    Composition {
        event_id,
        era: era_of(game).into(),
        include_extra,
        counted: members.len() as i32,
        roles,
        classes,
        armour,
        buffs,
        missing,
        missing_line,
    }
}

// ---- Loot: raid instances and famous items ------------------------------------

/// Raids you can pick a loot list for: (key, name, "classic"/"tbc", size).
pub const INSTANCES: &[(&str, &str, &str, i32)] = &[
    ("mc", "Molten Core", "classic", 40),
    ("ony", "Onyxia's Lair", "classic", 40),
    ("bwl", "Blackwing Lair", "classic", 40),
    ("zg", "Zul'Gurub", "classic", 20),
    ("aq20", "Ruins of Ahn'Qiraj (AQ20)", "classic", 20),
    ("aq40", "Temple of Ahn'Qiraj (AQ40)", "classic", 40),
    ("naxx", "Naxxramas", "classic", 40),
    ("kara", "Karazhan", "tbc", 10),
    ("gruul", "Gruul's Lair", "tbc", 25),
    ("mag", "Magtheridon's Lair", "tbc", 25),
    ("ssc", "Serpentshrine Cavern", "tbc", 25),
    ("tk", "Tempest Keep: The Eye", "tbc", 25),
    ("hyjal", "Battle for Mount Hyjal", "tbc", 25),
    ("bt", "Black Temple", "tbc", 25),
    ("za", "Zul'Aman", "tbc", 10),
    ("swp", "Sunwell Plateau", "tbc", 25),
];

/// One boss encounter on a raid's loot list. Wowhead files loot under the
/// NPC (or chest) that drops it, so an encounter lists every NPC, chest and
/// name it may appear as: Attumen and Midnight, the Opera event's
/// variants, Majordomo's Cache of the Firelord, the Four Horsemen Chest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Encounter {
    /// The name its section has on the list.
    pub name: &'static str,
    /// Its NPCs on Wowhead (Classic/TBC IDs).
    pub npcs: &'static [i32],
    /// The NPCs whose own pages list the loot ([] = the first NPC).
    pub loot_from: &'static [i32],
    /// The chest its loot is in (0 = none; the NPC drops it).
    pub chest: i32,
    /// Other names Wowhead shows for it (NPCs, chests).
    pub aliases: &'static [&'static str],
}

const fn boss(name: &'static str, npc: &'static [i32]) -> Encounter {
    Encounter {
        name,
        npcs: npc,
        loot_from: &[],
        chest: 0,
        aliases: &[],
    }
}

const fn group(
    name: &'static str,
    npcs: &'static [i32],
    loot_from: &'static [i32],
    chest: i32,
    aliases: &'static [&'static str],
) -> Encounter {
    Encounter {
        name,
        npcs,
        loot_from,
        chest,
        aliases,
    }
}

impl Encounter {
    /// The Wowhead pages its loot is listed on: ("object" | "npc", ID).
    pub fn loot_pages(&self) -> Vec<(&'static str, i32)> {
        if self.chest > 0 {
            return vec![("object", self.chest)];
        }
        let from = if self.loot_from.is_empty() {
            &self.npcs[..self.npcs.len().min(1)]
        } else {
            self.loot_from
        };
        from.iter().map(|&id| ("npc", id)).collect()
    }

    /// Whether Wowhead's NPC/chest ID or name is this encounter.
    pub fn matches(&self, id: i32, name: &str) -> bool {
        (id > 0 && (self.npcs.contains(&id) || self.chest == id))
            || self.name.eq_ignore_ascii_case(name.trim())
            || self
                .aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(name.trim()))
    }
}

/// Each listed raid's zone on Wowhead and its encounters in the usual kill
/// order (Wowhead lists loot in its own order; this puts the boss sections
/// in raid order, anything else goes after). Zone IDs are the game's own
/// area IDs, the same on every Wowhead branch (Classic, TBC, WoW: Forever).
pub const INSTANCE_ZONES: &[(&str, i32, &[Encounter])] = &[
    (
        "mc",
        2717,
        &[
            boss("Lucifron", &[12118]),
            boss("Magmadar", &[11982]),
            boss("Gehennas", &[12259]),
            boss("Garr", &[12057]),
            boss("Shazzrah", &[12264]),
            boss("Baron Geddon", &[12056]),
            boss("Sulfuron Harbinger", &[12098]),
            boss("Golemagg the Incinerator", &[11988]),
            group(
                "Majordomo Executus",
                &[12018],
                &[],
                179703,
                &["Cache of the Firelord"],
            ),
            boss("Ragnaros", &[11502]),
        ],
    ),
    ("ony", 2159, &[boss("Onyxia", &[10184])]),
    (
        "bwl",
        2677,
        &[
            boss("Razorgore the Untamed", &[12435]),
            boss("Vaelastrasz the Corrupt", &[13020]),
            boss("Broodlord Lashlayer", &[12017]),
            boss("Firemaw", &[11983]),
            boss("Ebonroc", &[14601]),
            boss("Flamegor", &[11981]),
            boss("Chromaggus", &[14020]),
            group(
                "Nefarian",
                &[11583, 10162],
                &[11583],
                0,
                &["Lord Victor Nefarius"],
            ),
        ],
    ),
    (
        "zg",
        1977,
        &[
            boss("High Priestess Jeklik", &[14517]),
            boss("High Priest Venoxis", &[14507]),
            boss("High Priestess Mar'li", &[14510]),
            group(
                "Bloodlord Mandokir",
                &[11382, 14988],
                &[11382],
                0,
                &["Ohgan"],
            ),
            boss("Gri'lek", &[15082]),
            boss("Hazza'rah", &[15083]),
            boss("Renataki", &[15084]),
            boss("Wushoolay", &[15085]),
            boss("Gahz'ranka", &[15114]),
            boss("High Priest Thekal", &[14509]),
            boss("High Priestess Arlokk", &[14515]),
            boss("Jin'do the Hexxer", &[11380]),
            boss("Hakkar", &[14834]),
        ],
    ),
    (
        "aq20",
        3429,
        &[
            boss("Kurinnaxx", &[15348]),
            boss("General Rajaxx", &[15341]),
            boss("Moam", &[15340]),
            boss("Buru the Gorger", &[15370]),
            boss("Ayamiss the Hunter", &[15369]),
            boss("Ossirian the Unscarred", &[15339]),
        ],
    ),
    (
        "aq40",
        3428,
        &[
            boss("The Prophet Skeram", &[15263]),
            group(
                "Bug Trio",
                &[15543, 15511, 15544],
                &[15543, 15511, 15544],
                0,
                &["Princess Yauj", "Lord Kri", "Vem", "Silithid Royalty"],
            ),
            boss("Battleguard Sartura", &[15516]),
            boss("Fankriss the Unyielding", &[15510]),
            boss("Viscidus", &[15299]),
            boss("Princess Huhuran", &[15509]),
            boss("Emperor Vek'lor", &[15276]),
            boss("Emperor Vek'nilash", &[15275]),
            boss("Ouro", &[15517]),
            group("C'Thun", &[15727, 15589], &[15727], 0, &["Eye of C'Thun"]),
        ],
    ),
    (
        "naxx",
        3456,
        &[
            boss("Anub'Rekhan", &[15956]),
            boss("Grand Widow Faerlina", &[15953]),
            boss("Maexxna", &[15952]),
            boss("Noth the Plaguebringer", &[15954]),
            boss("Heigan the Unclean", &[15936]),
            boss("Loatheb", &[16011]),
            boss("Instructor Razuvious", &[16061]),
            boss("Gothik the Harvester", &[16060]),
            group(
                "The Four Horsemen",
                &[16062, 16063, 16064, 16065],
                &[],
                181366,
                &[
                    "Four Horsemen Chest",
                    "Highlord Mograine",
                    "Sir Zeliek",
                    "Thane Korth'azz",
                    "Lady Blaumeux",
                ],
            ),
            boss("Patchwerk", &[16028]),
            boss("Grobbulus", &[15931]),
            boss("Gluth", &[15932]),
            group(
                "Thaddius",
                &[15928, 15929, 15930],
                &[15928],
                0,
                &["Stalagg", "Feugen"],
            ),
            boss("Sapphiron", &[15989]),
            boss("Kel'Thuzad", &[15990]),
        ],
    ),
    (
        "kara",
        3457,
        &[
            group(
                "Attumen the Huntsman",
                &[16152, 15550, 16151],
                &[16152],
                0,
                &["Midnight"],
            ),
            boss("Moroes", &[15687]),
            boss("Maiden of Virtue", &[16457]),
            group(
                "Opera Event",
                &[
                    17521, 17534, 17533, 18168, 17535, 17543, 17546, 17547, 17548,
                ],
                &[17521, 17534, 18168],
                0,
                &[
                    "The Big Bad Wolf",
                    "Romulo",
                    "Julianne",
                    "Romulo and Julianne",
                    "The Crone",
                    "Dorothee",
                    "Tito",
                    "Roar",
                    "Strawman",
                    "Tinhead",
                    "Opera",
                    "Opera House",
                ],
            ),
            boss("The Curator", &[15691]),
            boss("Terestian Illhoof", &[15688]),
            boss("Shade of Aran", &[16524]),
            boss("Netherspite", &[15689]),
            group(
                "Chess Event",
                &[16816],
                &[],
                185119,
                &["Dust Covered Chest", "Echo of Medivh"],
            ),
            boss("Prince Malchezaar", &[15690]),
            boss("Nightbane", &[17225]),
        ],
    ),
    (
        "gruul",
        3923,
        &[
            group(
                "High King Maulgar",
                &[18831, 18832, 18834, 18835, 18836],
                &[18831],
                0,
                &[
                    "Krosh Firehand",
                    "Olm the Summoner",
                    "Kiggler the Crazed",
                    "Blindeye the Seer",
                ],
            ),
            boss("Gruul the Dragonkiller", &[19044]),
        ],
    ),
    ("mag", 3836, &[boss("Magtheridon", &[17257])]),
    (
        "ssc",
        3607,
        &[
            boss("Hydross the Unstable", &[21216]),
            boss("The Lurker Below", &[21217]),
            group(
                "Leotheras the Blind",
                &[21215, 21875],
                &[21215],
                0,
                &["Shadow of Leotheras"],
            ),
            boss("Fathom-Lord Karathress", &[21214]),
            boss("Morogrim Tidewalker", &[21213]),
            boss("Lady Vashj", &[21212]),
        ],
    ),
    (
        "tk",
        3845,
        &[
            boss("Al'ar", &[19514]),
            boss("Void Reaver", &[19516]),
            boss("High Astromancer Solarian", &[18805]),
            group(
                "Kael'thas Sunstrider",
                &[19622],
                &[],
                0,
                // His weapons are NPCs in the fight; Wowhead files them so.
                &[
                    "Warp Slicer",
                    "Infinity Blades",
                    "Staff of Disintegration",
                    "Phaseshift Bulwark",
                    "Devastation",
                    "Cosmic Infuser",
                    "Netherstrand Longbow",
                ],
            ),
        ],
    ),
    (
        "hyjal",
        3606,
        &[
            boss("Rage Winterchill", &[17767]),
            boss("Anetheron", &[17808]),
            boss("Kaz'rogal", &[17888]),
            boss("Azgalor", &[17842]),
            boss("Archimonde", &[17968]),
        ],
    ),
    (
        "bt",
        3959,
        &[
            boss("High Warlord Naj'entus", &[22887]),
            boss("Supremus", &[22898]),
            boss("Shade of Akama", &[22841]),
            boss("Teron Gorefiend", &[22871]),
            boss("Gurtogg Bloodboil", &[22948]),
            group(
                "Reliquary of Souls",
                &[23420, 23418, 23419],
                &[23420],
                0,
                &[
                    "Essence of Anger",
                    "Essence of Suffering",
                    "Essence of Desire",
                ],
            ),
            boss("Mother Shahraz", &[22947]),
            group(
                "The Illidari Council",
                &[23426, 22949, 22950, 22951, 22952],
                &[22950],
                0,
                &[
                    "Gathios the Shatterer",
                    "High Nethermancer Zerevor",
                    "Lady Malande",
                    "Veras Darkshadow",
                ],
            ),
            boss("Illidan Stormrage", &[22917]),
        ],
    ),
    (
        "za",
        3805,
        &[
            boss("Nalorakk", &[23576]),
            boss("Akil'zon", &[23574]),
            boss("Jan'alai", &[23578]),
            boss("Halazzi", &[23577]),
            boss("Hex Lord Malacrass", &[24239]),
            boss("Zul'jin", &[23863]),
            group(
                "Timed chest",
                &[],
                &[],
                0,
                &[
                    "Tanzar's Trunk",
                    "Kasha's Bag",
                    "Ashli's Bag",
                    "Harkor's Satchel",
                    "Hazlek's Trunk",
                ],
            ),
        ],
    ),
    (
        "swp",
        4075,
        &[
            group(
                "Kalecgos",
                &[24850, 24892, 24891],
                &[24850],
                0,
                &["Sathrovarr the Corruptor"],
            ),
            boss("Brutallus", &[24882]),
            boss("Felmyst", &[25038]),
            group(
                "Eredar Twins",
                &[25165, 25166],
                &[25165, 25166],
                0,
                &["Lady Sacrolash", "Grand Warlock Alythess"],
            ),
            group("M'uru", &[25741, 25840], &[25840], 0, &["Entropius"]),
            boss("Kil'jaeden", &[25315]),
        ],
    ),
];

/// A listed raid's Wowhead zone ID.
pub fn instance_zone(key: &str) -> Option<i32> {
    INSTANCE_ZONES.iter().find(|z| z.0 == key).map(|z| z.1)
}

/// The listed raid whose Wowhead zone this is.
pub fn instance_of_zone(zone: i32) -> Option<&'static str> {
    INSTANCE_ZONES.iter().find(|z| z.1 == zone).map(|z| z.0)
}

/// A listed raid's encounters in kill order ([] if it isn't listed).
pub fn encounters(key: &str) -> &'static [Encounter] {
    INSTANCE_ZONES
        .iter()
        .find(|z| z.0 == key)
        .map(|z| z.2)
        .unwrap_or(&[])
}

/// The encounter Wowhead's NPC/chest (ID and name) belongs to in a listed
/// raid, with its place in the kill order.
pub fn encounter_for(key: &str, id: i32, name: &str) -> Option<(usize, &'static Encounter)> {
    let list = encounters(key);
    // By ID first (two NPCs can share a name across versions).
    list.iter()
        .position(|e| id > 0 && (e.npcs.contains(&id) || e.chest == id))
        .or_else(|| list.iter().position(|e| e.matches(0, name)))
        .map(|i| (i, &list[i]))
}

/// Where a boss comes in a listed raid (unknown bosses after, in the order
/// Wowhead gave them).
pub fn boss_rank(key: &str, boss: &str) -> Option<usize> {
    encounter_for(key, 0, boss).map(|e| e.0)
}

/// A few famous items per raid so the lists work out of the box:
/// (raid, item ID, name, quality, boss). IDs are Wowhead's. Leaders add the
/// rest themselves.
pub const SEED_ITEMS: &[(&str, i32, &str, i32, &str)] = &[
    ("mc", 17204, "Eye of Sulfuras", 5, "Ragnaros"),
    ("mc", 18814, "Choker of the Fire Lord", 4, "Ragnaros"),
    ("mc", 17076, "Bonereaver's Edge", 4, "Ragnaros"),
    ("mc", 17106, "Malistar's Defender", 4, "Ragnaros"),
    ("mc", 16922, "Leggings of Transcendence", 4, "Ragnaros"),
    // The left binding drops from Baron Geddon, the right one from Garr
    // (both are called "Bindings of the Windseeker" in game).
    ("mc", 18563, "Bindings of the Windseeker", 5, "Baron Geddon"),
    ("mc", 18564, "Bindings of the Windseeker", 5, "Garr"),
    ("mc", 17069, "Striker's Mark", 4, "Magmadar"),
    (
        "mc",
        17103,
        "Azuresong Mageblade",
        4,
        "Golemagg the Incinerator",
    ),
    (
        "mc",
        18842,
        "Staff of Dominance",
        4,
        "Golemagg the Incinerator",
    ),
    ("ony", 18423, "Head of Onyxia", 4, "Onyxia"),
    ("ony", 17078, "Sapphiron Drape", 4, "Onyxia"),
    ("ony", 18205, "Eskhandar's Collar", 4, "Onyxia"),
    ("ony", 17075, "Vis'kag the Bloodletter", 4, "Onyxia"),
    ("ony", 17068, "Deathbringer", 4, "Onyxia"),
    ("ony", 18813, "Ring of Binding", 4, "Onyxia"),
    (
        "bwl",
        19364,
        "Ashkandi, Greatsword of the Brotherhood",
        4,
        "Nefarian",
    ),
    ("bwl", 19003, "Head of Nefarian", 4, "Nefarian"),
    ("bwl", 19360, "Lok'amir il Romathis", 4, "Nefarian"),
    ("bwl", 19356, "Staff of the Shadow Flame", 4, "Nefarian"),
    ("bwl", 19379, "Neltharion's Tear", 4, "Nefarian"),
    ("bwl", 19406, "Drake Fang Talisman", 4, "Chromaggus"),
    (
        "bwl",
        19352,
        "Chromatically Tempered Sword",
        4,
        "Chromaggus",
    ),
    ("zg", 19902, "Swift Zulian Tiger", 4, "High Priest Thekal"),
    (
        "zg",
        19872,
        "Swift Razzashi Raptor",
        4,
        "Bloodlord Mandokir",
    ),
    ("zg", 19802, "Heart of Hakkar", 4, "Hakkar"),
    ("zg", 19859, "Fang of the Faceless", 4, "Hakkar"),
    ("zg", 19864, "Bloodcaller", 4, "Hakkar"),
    ("zg", 19865, "Warblade of the Hakkari", 4, "Hakkar"),
    (
        "aq20",
        21220,
        "Head of Ossirian the Unscarred",
        4,
        "Ossirian the Unscarred",
    ),
    (
        "aq20",
        21452,
        "Staff of the Ruins",
        4,
        "Ossirian the Unscarred",
    ),
    ("aq20", 21466, "Stinger of Ayamiss", 4, "Ayamiss the Hunter"),
    ("aq40", 21134, "Dark Edge of Insanity", 4, "C'Thun"),
    ("aq40", 21126, "Death's Sting", 4, "C'Thun"),
    ("aq40", 21221, "Eye of C'Thun", 4, "C'Thun"),
    (
        "aq40",
        21128,
        "Staff of the Qiraji Prophets",
        4,
        "The Prophet Skeram",
    ),
    ("aq40", 21218, "Blue Qiraji Resonating Crystal", 4, ""),
    (
        "naxx",
        22691,
        "Corrupted Ashbringer",
        4,
        "The Four Horsemen",
    ),
    ("naxx", 23054, "Gressil, Dawn of Ruin", 4, "Kel'Thuzad"),
    ("naxx", 23577, "The Hungering Cold", 4, "Kel'Thuzad"),
    ("naxx", 22798, "Might of Menethil", 4, "Kel'Thuzad"),
    ("naxx", 22799, "Soulseeker", 4, "Kel'Thuzad"),
    (
        "naxx",
        22520,
        "The Phylactery of Kel'Thuzad",
        4,
        "Kel'Thuzad",
    ),
    (
        "kara",
        30480,
        "Fiery Warhorse's Reins",
        4,
        "Attumen the Huntsman",
    ),
    ("kara", 28770, "Nathrezim Mindblade", 4, "Prince Malchezaar"),
    ("kara", 28773, "Gorehowl", 4, "Prince Malchezaar"),
    (
        "gruul",
        28830,
        "Dragonspine Trophy",
        4,
        "Gruul the Dragonkiller",
    ),
    (
        "gruul",
        28794,
        "Axe of the Gronn Lords",
        4,
        "Gruul the Dragonkiller",
    ),
    ("mag", 28789, "Eye of Magtheridon", 4, "Magtheridon"),
    ("mag", 28782, "Crystalheart Pulse-Staff", 4, "Magtheridon"),
    ("ssc", 30103, "Fang of Vashj", 4, "Lady Vashj"),
    ("tk", 32458, "Ashes of Al'ar", 4, "Kael'thas Sunstrider"),
    (
        "tk",
        29996,
        "Rod of the Sun King",
        4,
        "Kael'thas Sunstrider",
    ),
    ("hyjal", 30902, "Cataclysm's Edge", 4, "Archimonde"),
    ("hyjal", 30910, "Tempest of Chaos", 4, "Archimonde"),
    ("bt", 32837, "Warglaive of Azzinoth", 5, "Illidan Stormrage"),
    // Main hand and off hand share the name.
    ("bt", 32838, "Warglaive of Azzinoth", 5, "Illidan Stormrage"),
    (
        "bt",
        32336,
        "Black Bow of the Betrayer",
        4,
        "Illidan Stormrage",
    ),
    ("za", 33809, "Amani War Bear", 4, "Timed chest"),
    ("swp", 34334, "Thori'dal, the Stars' Fury", 5, "Kil'jaeden"),
    ("swp", 34247, "Apolyon, the Soul-Render", 4, "Kil'jaeden"),
];

/// The listed raid with this key, if it is one.
pub fn instance(key: &str) -> Option<(&'static str, &'static str, &'static str, i32)> {
    INSTANCES.iter().find(|i| i.0 == key).copied()
}

/// What a raid's loot list is filed under: a listed raid's key, else the
/// leader's own name for it in lower case ("" = none).
pub fn instance_key(raw: &str) -> String {
    let t = raw.trim();
    if let Some(i) = INSTANCES
        .iter()
        .find(|i| i.0.eq_ignore_ascii_case(t) || i.1.eq_ignore_ascii_case(t))
    {
        return i.0.to_string();
    }
    t.to_lowercase()
}

/// The name to show for a raid's `loot_instance`.
pub fn instance_name(key: &str) -> String {
    instance(key)
        .map(|i| i.1.to_string())
        .unwrap_or_else(|| key.to_string())
}

/// A raid you can pick in Plan/Edit raid (listed, or one this outhouse has
/// used before).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LootInstance {
    pub key: String,
    pub name: String,
    /// "classic", "tbc" or "" (an outhouse's own).
    pub era: String,
    pub size: i32,
    pub builtin: bool,
}

pub fn builtin_instances() -> Vec<LootInstance> {
    INSTANCES
        .iter()
        .map(|&(key, name, era, size)| LootInstance {
            key: key.into(),
            name: name.into(),
            era: era.into(),
            size,
            builtin: true,
        })
        .collect()
}

/// An item on a raid's loot list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LootItem {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    /// Who drops it ("" = trash or unknown).
    #[serde(default)]
    pub boss: String,
    /// Added by a raid leader in this outhouse (they can remove it).
    #[serde(default)]
    pub custom: bool,
    /// Wowhead's icon name ("" if unknown); the picture is at
    /// `/api/wow/item-icons/{item_id}`.
    #[serde(default)]
    pub icon: String,
    /// "Two-Hand Sword", "Plate Chest", "Trinket", … ("" if unknown).
    #[serde(default)]
    pub slot: String,
    /// "weapon", "armour", "trinket", "ring", "neck", "back" or "other".
    #[serde(default)]
    pub category: String,
    /// "cloth", "leather", "mail", "plate" or "".
    #[serde(default)]
    pub armour: String,
    /// Which classes it suits ([`class_bit`] bits; 0 = unknown, anyone).
    #[serde(default)]
    pub classes: u32,
    /// Drop chance in percent, when Wowhead has one.
    #[serde(default)]
    pub chance: Option<f32>,
    /// A trash drop (not from a boss).
    #[serde(default)]
    pub trash: bool,
}

/// A Wowhead page a raid's loot list is read from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LootSource {
    /// "zone", "npc" or "object".
    pub kind: String,
    pub id: i32,
    /// Wowhead's branch it was read from ("classic", "tbc", … "" = retail).
    pub branch: String,
    pub name: String,
    /// The page on Wowhead.
    pub url: String,
    /// The raid's own zone (not removable); else imported by a raid leader.
    pub builtin: bool,
    /// "ok", "pending" (not read yet), "missing" (Wowhead doesn't have it)
    /// or "error" (Wowhead didn't answer; the saved list is shown).
    pub status: String,
    pub items: usize,
    #[serde(default)]
    pub fetched_at: Option<DateTime<Utc>>,
}

/// `GET /api/events/{id}/loot-table`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LootTable {
    pub instance: String,
    pub instance_name: String,
    pub items: Vec<LootItem>,
    /// Bosses in raid order (for the picker's sections).
    #[serde(default)]
    pub bosses: Vec<String>,
    #[serde(default)]
    pub sources: Vec<LootSource>,
    /// Item links point here: `{item_base}/item={id}`.
    #[serde(default)]
    pub item_base: String,
    /// …which is this Wowhead branch ("classic", "tbc", …).
    #[serde(default)]
    pub item_branch: String,
    /// Still being read from Wowhead (ask again in a few seconds).
    #[serde(default)]
    pub refreshing: bool,
    /// Wowhead couldn't be read, so this is the built-in short list.
    #[serde(default)]
    pub offline: bool,
}

/// `POST /api/events/{id}/loot-sources` (raid leaders): read a raid's loot
/// from a Wowhead zone, NPC or object (chest) link.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LootSourceRequest {
    pub url: String,
}

// ---- Item details from Wowhead's numbers ---------------------------------

/// Classes as bits (Blizzard's class IDs: bit `id - 1`).
pub const CLASS_BITS: &[(&str, u32)] = &[
    ("Warrior", 1),
    ("Paladin", 2),
    ("Hunter", 4),
    ("Rogue", 8),
    ("Priest", 16),
    ("Shaman", 64),
    ("Mage", 128),
    ("Warlock", 256),
    ("Druid", 1024),
];
pub const ALL_CLASSES: u32 = 1503;

pub fn class_bit(class: &str) -> u32 {
    CLASS_BITS
        .iter()
        .find(|c| c.0.eq_ignore_ascii_case(class))
        .map(|c| c.1)
        .unwrap_or(0)
}

fn bits(classes: &[&str]) -> u32 {
    classes.iter().map(|c| class_bit(c)).fold(0, |a, b| a | b)
}

/// Inventory slot names (Wowhead's `slot`).
fn slot_name(slot: i32) -> &'static str {
    match slot {
        1 => "Head",
        2 => "Neck",
        3 => "Shoulder",
        4 => "Shirt",
        5 | 20 => "Chest",
        6 => "Waist",
        7 => "Legs",
        8 => "Feet",
        9 => "Wrist",
        10 => "Hands",
        11 => "Ring",
        12 => "Trinket",
        13 => "One-Hand",
        14 => "Shield",
        15 | 26 => "Ranged",
        16 => "Back",
        17 => "Two-Hand",
        18 => "Bag",
        19 => "Tabard",
        21 => "Main Hand",
        22 => "Off Hand",
        23 => "Held In Off-hand",
        25 => "Thrown",
        28 => "Relic",
        _ => "",
    }
}

fn weapon_name(subclass: i32) -> &'static str {
    match subclass {
        0 | 1 => "Axe",
        2 => "Bow",
        3 => "Gun",
        4 | 5 => "Mace",
        6 => "Polearm",
        7 | 8 => "Sword",
        10 => "Staff",
        13 => "Fist Weapon",
        15 => "Dagger",
        16 => "Thrown",
        18 => "Crossbow",
        19 => "Wand",
        20 => "Fishing Pole",
        _ => "",
    }
}

/// Armour type of an armour item ("" for the rest).
pub fn armour_type(item_class: i32, subclass: i32, slot: i32) -> &'static str {
    // Cloaks are "cloth" to the game, but everyone wears them.
    if item_class != 4 || matches!(slot, 2 | 11 | 12 | 16 | 23) {
        return "";
    }
    match subclass {
        1 => "cloth",
        2 => "leather",
        3 => "mail",
        4 => "plate",
        _ => "",
    }
}

/// The picker's filter group for an item.
pub fn item_category(item_class: i32, subclass: i32, slot: i32) -> &'static str {
    match slot {
        12 => return "trinket",
        11 => return "ring",
        2 => return "neck",
        16 => return "back",
        13 | 14 | 15 | 17 | 21 | 22 | 23 | 25 | 26 | 28 => return "weapon",
        1 | 3 | 5 | 6 | 7 | 8 | 9 | 10 | 20 => {
            if item_class == 4 || item_class < 0 {
                return "armour";
            }
        }
        _ => {}
    }
    match item_class {
        2 => "weapon",
        4 if (1..=4).contains(&subclass) && slot == 0 => "armour",
        4 if (6..=9).contains(&subclass) => "weapon",
        _ => "other",
    }
}

/// "Two-Hand Sword", "Plate Chest", "Ring", "Mount", … ("" if unknown).
pub fn slot_label(item_class: i32, subclass: i32, slot: i32) -> String {
    let s = slot_name(slot);
    match item_class {
        2 => {
            let w = weapon_name(subclass);
            if w.is_empty() {
                s.to_string()
            } else if s.is_empty()
                || matches!(slot, 15 | 25 | 26)
                || matches!(w, "Staff" | "Polearm" | "Wand" | "Fishing Pole")
            {
                w.to_string()
            } else {
                format!("{s} {w}")
            }
        }
        4 => match (subclass, slot) {
            (6, _) => "Shield".into(),
            (7, _) => "Libram".into(),
            (8, _) => "Idol".into(),
            (9, _) => "Totem".into(),
            (_, 2 | 11 | 12 | 16 | 23) => s.into(),
            (1..=4, _) if !s.is_empty() => {
                let t = armour_type(item_class, subclass, slot);
                let mut c = t.chars();
                let t = c
                    .next()
                    .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                    .unwrap_or_default();
                format!("{t} {s}")
            }
            _ => s.into(),
        },
        0 => "Consumable".into(),
        7 => "Trade goods".into(),
        9 => "Recipe".into(),
        12 => "Quest item".into(),
        15 if subclass == 5 => "Mount".into(),
        15 if subclass == 2 => "Pet".into(),
        15 => "Token".into(),
        _ => s.into(),
    }
}

/// Which classes an item suits: who can use the weapon, who wears the
/// armour type (a Warrior can wear cloth, but won't reserve it), anyone for
/// jewellery, cloaks and tokens; narrowed by the item's own class list.
pub fn suited_classes(item_class: i32, subclass: i32, slot: i32, req_class: i32) -> u32 {
    let base = match item_class {
        2 => match subclass {
            0 | 1 => bits(&["Warrior", "Paladin", "Hunter", "Shaman"]),
            2 | 3 | 16 | 18 => bits(&["Warrior", "Hunter", "Rogue"]),
            4 => bits(&["Warrior", "Paladin", "Rogue", "Priest", "Shaman", "Druid"]),
            5 => bits(&["Warrior", "Paladin", "Shaman", "Druid"]),
            6 => bits(&["Warrior", "Paladin", "Hunter"]),
            7 => bits(&["Warrior", "Paladin", "Hunter", "Rogue", "Mage", "Warlock"]),
            8 => bits(&["Warrior", "Paladin", "Hunter"]),
            10 => bits(&[
                "Warrior", "Hunter", "Priest", "Shaman", "Mage", "Warlock", "Druid",
            ]),
            13 => bits(&["Warrior", "Hunter", "Rogue", "Shaman", "Druid"]),
            15 => ALL_CLASSES & !bits(&["Paladin"]),
            19 => bits(&["Priest", "Mage", "Warlock"]),
            _ => ALL_CLASSES,
        },
        4 => match (subclass, slot) {
            (6, _) => bits(&["Warrior", "Paladin", "Shaman"]),
            (7, _) => bits(&["Paladin"]),
            (8, _) => bits(&["Druid"]),
            (9, _) => bits(&["Shaman"]),
            _ => match armour_type(item_class, subclass, slot) {
                "cloth" => bits(&["Priest", "Mage", "Warlock"]),
                "leather" => bits(&["Rogue", "Druid"]),
                "mail" => bits(&["Hunter", "Shaman"]),
                "plate" => bits(&["Warrior", "Paladin"]),
                _ => ALL_CLASSES,
            },
        },
        _ => ALL_CLASSES,
    };
    let req = (req_class.max(0) as u32) & ALL_CLASSES;
    if req != 0 {
        base & req
    } else {
        base
    }
}

/// Someone's soft reserve on a raid (shown to everyone who can see it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RaidReserve {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub item_id: i32,
    pub item_name: String,
    pub quality: i32,
    pub reserved_at: DateTime<Utc>,
}

/// `PUT /api/events/{id}/reserves` adds one of your reserves (by item ID,
/// Wowhead link or name); `DELETE /api/events/{id}/reserves/{item}` takes
/// it away (raid leaders: `?user_id=` for someone else's).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReserveRequest {
    #[serde(default)]
    pub item_id: Option<i32>,
    #[serde(default)]
    pub query: Option<String>,
}

/// `PUT /api/events/{id}/reserves/lock`: true locks, false unlocks, null
/// goes back to the raid's automatic lock.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReserveLockRequest {
    #[serde(default)]
    pub locked: Option<bool>,
}

/// `POST /api/events/{id}/loot-items` (raid leaders): adds an item to the
/// raid's loot list for the whole outhouse.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LootItemRequest {
    pub query: String,
}

/// `POST /api/events/{id}/loot`: someone won an item.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AwardRequest {
    #[serde(with = "as_string")]
    pub user_id: i64,
    #[serde(default)]
    pub item_id: Option<i32>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub note: String,
}

/// An item someone won (the loot log).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LootAward {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "option_as_string", default)]
    pub event_id: Option<i64>,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub raid_title: String,
    pub raid_starts_at: DateTime<Utc>,
    pub loot_instance: String,
    pub instance_name: String,
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    /// Their character on the raid's sign-up, if they had one.
    pub character_name: String,
    pub item_id: i32,
    pub item_name: String,
    pub quality: i32,
    /// They had soft-reserved it.
    pub reserved: bool,
    pub note: String,
    #[serde(with = "option_as_string", default)]
    pub awarded_by: Option<i64>,
    pub awarded_at: DateTime<Utc>,
    /// The Wowhead branch its item links go to ("classic", "tbc", …).
    #[serde(default)]
    pub branch: String,
}

/// An item from the lookup (`GET /api/wow/items?q=` / `/api/wow/items/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemLookup {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    /// Picture URL ("" if none).
    pub icon_url: String,
    /// Tooltip lines.
    pub tooltip: String,
    pub url: String,
    /// The tooltip as Wowhead shows it: rows with a right-hand part
    /// ("Two-Hand" … "Sword") and a colour ("q2" green Equip: lines, …).
    #[serde(default)]
    pub lines: Vec<TipLine>,
}

/// One row of an item tooltip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TipLine {
    pub left: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub right: String,
    /// Wowhead's colour class: "q0" … "q7", "q" (yellow) or "" (white).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub q: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, class: &str, spec: &str, role: &str) -> CompMember {
        CompMember {
            user_id: 1,
            name: name.into(),
            class: class.into(),
            spec: spec.into(),
            role: role.into(),
        }
    }

    #[test]
    fn finds_missing_roles_classes_and_dispels() {
        let people = [
            m("Tank", "Warrior", "Protection", "tank"),
            m("Heals", "Priest", "Holy", "healer"),
            m("Pally", "Paladin", "Holy", "healer"),
            m("Stabby", "Rogue", "Combat", "dps"),
            m("Pew", "Hunter", "Marksmanship", "dps"),
        ];
        let c = compose(
            1,
            &people,
            "forever",
            [("tank", 1), ("healer", 3), ("dps", 3)],
            false,
        );
        assert_eq!(c.counted, 5);
        assert_eq!(c.roles[1].missing, 1);
        assert_eq!(c.roles[2].missing, 1);
        let get = |k: &str| c.buffs.iter().find(|b| b.key == k).unwrap();
        assert!(get("fortitude").covered);
        assert_eq!(get("fortitude").providers, vec!["Heals"]);
        assert!(get("trueshot_aura").covered);
        assert!(!get("arcane_intellect").covered);
        assert!(!get("decurse").covered);
        assert!(get("cure_poison").covered); // the Paladin
        assert!(c.buffs.iter().all(|b| b.key != "bloodlust"));
        assert_eq!(
            c.missing_line,
            "Need: 1 healer · 1 DPS · No Druid (Mark of the Wild, battle res) · No Mage (Arcane Intellect) · No Shaman (Windfury) · No Warlock (Curses, Healthstones) · No decurse"
        );
        assert_eq!(
            c.armour.iter().map(|a| a.count).collect::<Vec<_>>(),
            vec![1, 1, 1, 2]
        );
        assert_eq!(c.classes.len(), 5);
    }

    #[test]
    fn tbc_has_its_own_buffs() {
        let people = [m("Totems", "Shaman", "Restoration", "healer")];
        let c = compose(
            1,
            &people,
            "tbc",
            [("tank", 0), ("healer", 1), ("dps", 0)],
            false,
        );
        assert!(c.buffs.iter().any(|b| b.key == "bloodlust" && b.covered));
        assert!(c.buffs.iter().any(|b| b.key == "drums" && !b.covered));
        assert!(c.buffs.iter().all(|b| b.key != "tranq_shot"));
        assert!(!c.missing_line.contains("Shaman"));
    }

    #[test]
    fn instances_and_seeds_line_up() {
        for s in SEED_ITEMS {
            assert!(instance(s.0).is_some(), "{} has no raid", s.0);
            assert!((0..=7).contains(&s.3));
        }
        for i in INSTANCES {
            assert!(
                SEED_ITEMS.iter().any(|s| s.0 == i.0),
                "{} has no items",
                i.0
            );
        }
        assert_eq!(instance_key("Molten Core"), "mc");
        assert_eq!(instance_key(" MC "), "mc");
        assert_eq!(instance_key("Scarlet Raid"), "scarlet raid");
        assert_eq!(instance_name("bwl"), "Blackwing Lair");
        for i in INSTANCES {
            assert!(instance_zone(i.0).is_some(), "{} has no zone", i.0);
        }
        assert_eq!(instance_zone("mc"), Some(2717));
        assert_eq!(instance_zone("swp"), Some(4075));
        assert_eq!(boss_rank("mc", "ragnaros"), Some(9));
        assert_eq!(boss_rank("mc", "Onyxia"), None);
        assert_eq!(boss_rank("mc", "Cache of the Firelord"), Some(8));
        assert_eq!(instance_of_zone(3457), Some("kara"));
        // Multi-NPC encounters, by ID or by any of their names.
        let (i, e) = encounter_for("kara", 16151, "Midnight").unwrap();
        assert_eq!((i, e.name), (0, "Attumen the Huntsman"));
        assert_eq!(
            encounter_for("kara", 0, "Julianne").unwrap().1.name,
            "Opera Event"
        );
        assert_eq!(
            encounter_for("kara", 18168, "").unwrap().1.name,
            "Opera Event"
        );
        assert_eq!(
            encounter_for("kara", 185119, "").unwrap().1.name,
            "Chess Event"
        );
        assert_eq!(
            encounter_for("naxx", 0, "Four Horsemen Chest")
                .unwrap()
                .1
                .name,
            "The Four Horsemen"
        );
        assert_eq!(
            encounter_for("tk", 0, "Warp Slicer").unwrap().1.name,
            "Kael'thas Sunstrider"
        );
        assert!(encounter_for("mc", 228438, "x").is_none());
        assert_eq!(encounters("kara")[0].loot_pages(), vec![("npc", 16152)]);
        assert_eq!(encounters("mc")[8].loot_pages(), vec![("object", 179703)]);
        assert_eq!(encounters("ony")[0].loot_pages(), vec![("npc", 10184)]);
        // Every built-in item's boss is one of its raid's encounters.
        for s in SEED_ITEMS {
            assert!(
                s.4.is_empty() || boss_rank(s.0, s.4).is_some(),
                "{}: {}",
                s.0,
                s.4
            );
        }
        for z in INSTANCE_ZONES {
            for e in z.2 {
                assert!(
                    e.npcs.iter().chain([&e.chest]).all(|&id| id < 200_000),
                    "{}",
                    e.name
                );
            }
        }
    }

    #[test]
    fn item_details() {
        // Bonereaver's Edge: two-hand sword.
        assert_eq!(slot_label(2, 8, 17), "Two-Hand Sword");
        assert_eq!(item_category(2, 8, 17), "weapon");
        let s = suited_classes(2, 8, 17, 0);
        assert!(s & class_bit("Warrior") != 0 && s & class_bit("Mage") == 0);
        // Leggings of Transcendence: cloth legs, priests only.
        assert_eq!(slot_label(4, 1, 7), "Cloth Legs");
        assert_eq!(armour_type(4, 1, 7), "cloth");
        assert_eq!(suited_classes(4, 1, 7, 16), class_bit("Priest"));
        assert_eq!(item_category(4, 1, 7), "armour");
        // Cloaks, rings, trinkets: anyone.
        assert_eq!(slot_label(4, 1, 16), "Back");
        assert_eq!(item_category(4, 1, 16), "back");
        assert_eq!(suited_classes(4, 1, 16, 0), ALL_CLASSES);
        assert_eq!(item_category(4, 0, 12), "trinket");
        assert_eq!(item_category(4, 0, 11), "ring");
        assert_eq!(slot_label(4, 0, 2), "Neck");
        assert_eq!(slot_label(2, 2, 15), "Bow");
        assert_eq!(slot_label(2, 10, 17), "Staff");
        assert_eq!(slot_label(4, 6, 14), "Shield");
        assert_eq!(slot_label(15, 5, 0), "Mount");
        assert_eq!(item_category(15, 5, 0), "other");
        assert_eq!(
            suited_classes(2, 19, 26, 0),
            bits(&["Priest", "Mage", "Warlock"])
        );
        assert_eq!(class_bit("druid"), 1024);
        assert_eq!(CLASS_BITS.iter().map(|c| c.1).sum::<u32>(), ALL_CLASSES);
    }
}
