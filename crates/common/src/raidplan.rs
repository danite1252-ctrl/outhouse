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

/// A few famous items per raid so the lists work out of the box:
/// (raid, item ID, name, quality, boss). IDs are Wowhead's. Leaders add the
/// rest themselves.
pub const SEED_ITEMS: &[(&str, i32, &str, i32, &str)] = &[
    ("mc", 17204, "Eye of Sulfuras", 5, "Ragnaros"),
    ("mc", 18814, "Choker of the Fire Lord", 4, "Ragnaros"),
    ("mc", 17076, "Bonereaver's Edge", 4, "Ragnaros"),
    ("mc", 17106, "Malistar's Defender", 4, "Ragnaros"),
    ("mc", 16922, "Leggings of Transcendence", 4, "Ragnaros"),
    ("mc", 18563, "Bindings of the Windseeker (left)", 5, "Garr"),
    (
        "mc",
        18564,
        "Bindings of the Windseeker (right)",
        5,
        "Baron Geddon",
    ),
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
    ("aq40", 21128, "Staff of the Qiraji Prophets", 4, ""),
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
    (
        "bt",
        32838,
        "Warglaive of Azzinoth (off hand)",
        5,
        "Illidan Stormrage",
    ),
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LootItem {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    #[serde(default)]
    pub boss: String,
    /// Added by a raid leader in this outhouse (they can remove it).
    #[serde(default)]
    pub custom: bool,
}

/// `GET /api/events/{id}/loot-table`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LootTable {
    pub instance: String,
    pub instance_name: String,
    pub items: Vec<LootItem>,
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
    }
}
