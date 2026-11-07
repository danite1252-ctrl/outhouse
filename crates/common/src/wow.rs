//! World of Warcraft (Classic era / WoW: Forever) data and the shapes used
//! by the group finder, characters and Wowhead lookups.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::as_string;

/// The nine classes of the original game (WoW: Forever has no others).
pub const CLASSES: [&str; 9] = [
    "Druid", "Hunter", "Mage", "Paladin", "Priest", "Rogue", "Shaman", "Warlock", "Warrior",
];

/// Professions, primary then secondary.
pub const PROFESSIONS: [&str; 12] = [
    "Alchemy",
    "Blacksmithing",
    "Enchanting",
    "Engineering",
    "Herbalism",
    "Leatherworking",
    "Mining",
    "Skinning",
    "Tailoring",
    "Cooking",
    "Fishing",
    "First Aid",
];

/// Level cap in WoW: Forever and Classic Era.
pub const MAX_LEVEL: i32 = 60;
/// Highest level cap of any game we know (TBC: 70).
pub const TOP_LEVEL: i32 = 70;

// ---- Games -------------------------------------------------------------------

/// The games characters can be from: WoW: Forever (realmless, first and last
/// names, rulesets), Burning Crusade Classic / Anniversary, and Classic Era.
pub const GAMES: [&str; 3] = ["forever", "tbc", "era"];
pub const DEFAULT_GAME: &str = "forever";

/// WoW: Forever rulesets (instead of realms).
pub const RULESETS: [&str; 4] = ["Normal", "PvP", "Roleplay", "Hardcore"];

pub const CLASSIC_RACES: [&str; 8] = [
    "Human",
    "Dwarf",
    "Night Elf",
    "Gnome",
    "Orc",
    "Undead",
    "Tauren",
    "Troll",
];
pub const TBC_RACES: [&str; 10] = [
    "Human",
    "Dwarf",
    "Night Elf",
    "Gnome",
    "Draenei",
    "Orc",
    "Undead",
    "Tauren",
    "Troll",
    "Blood Elf",
];

/// The canonical id of a game ("WoW: Forever" → "forever"), if it is one.
pub fn game_id(raw: &str) -> Option<&'static str> {
    let k: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    Some(match k.as_str() {
        "" | "forever" | "wowforever" => "forever",
        "tbc" | "bc" | "burningcrusade" | "tbcclassic" | "tbcanniversary" => "tbc",
        "era" | "classicera" | "classic" | "vanilla" => "era",
        _ => return None,
    })
}

pub fn game_label(game: &str) -> &'static str {
    match game {
        "tbc" => "TBC",
        "era" => "Classic Era",
        "forever" => "WoW: Forever",
        _ => "",
    }
}

pub fn max_level_for(game: &str) -> i32 {
    if game == "tbc" {
        70
    } else {
        MAX_LEVEL
    }
}

pub fn professions_for(game: &str) -> Vec<&'static str> {
    let mut list: Vec<&'static str> = PROFESSIONS.to_vec();
    if game == "tbc" {
        // Jewelcrafting sits with the other primary professions.
        list.insert(4, "Jewelcrafting");
    }
    list
}

pub fn races_for(game: &str) -> &'static [&'static str] {
    if game == "tbc" {
        &TBC_RACES
    } else {
        &CLASSIC_RACES
    }
}

/// What the character form needs to know about a game.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GameInfo {
    pub id: String,
    pub name: String,
    pub max_level: i32,
    /// First and last names plus a ruleset (WoW: Forever), instead of one
    /// name and a realm.
    pub two_names: bool,
    pub rulesets: Vec<String>,
    pub professions: Vec<String>,
    pub races: Vec<String>,
}

pub fn games() -> Vec<GameInfo> {
    GAMES
        .iter()
        .map(|&g| GameInfo {
            id: g.into(),
            name: game_label(g).into(),
            max_level: max_level_for(g),
            two_names: g == "forever",
            rulesets: if g == "forever" {
                RULESETS.iter().map(|r| r.to_string()).collect()
            } else {
                Vec::new()
            },
            professions: professions_for(g).iter().map(|p| p.to_string()).collect(),
            races: races_for(g).iter().map(|r| r.to_string()).collect(),
        })
        .collect()
}

/// The canonical spelling of a class name ("paladin" → "Paladin"), if it is one.
pub fn class_name(raw: &str) -> Option<&'static str> {
    let raw = raw.trim();
    CLASSES
        .iter()
        .find(|c| c.eq_ignore_ascii_case(raw))
        .copied()
}

/// A dungeon or raid people group up for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Activity {
    pub name: String,
    /// "dungeon" or "raid".
    pub kind: String,
    pub min_level: i32,
    pub max_level: i32,
    /// Group size.
    pub size: i32,
}

/// Classic-era dungeons and raids: (name, kind, min level, max level, size).
pub const ACTIVITIES: &[(&str, &str, i32, i32, i32)] = &[
    ("Ragefire Chasm", "dungeon", 13, 18, 5),
    ("Wailing Caverns", "dungeon", 17, 24, 5),
    ("The Deadmines", "dungeon", 17, 26, 5),
    ("Shadowfang Keep", "dungeon", 22, 30, 5),
    ("Blackfathom Deeps", "dungeon", 24, 32, 5),
    ("The Stockade", "dungeon", 24, 32, 5),
    ("Gnomeregan", "dungeon", 29, 38, 5),
    ("Razorfen Kraul", "dungeon", 29, 38, 5),
    ("Scarlet Monastery: Graveyard", "dungeon", 26, 36, 5),
    ("Scarlet Monastery: Library", "dungeon", 29, 39, 5),
    ("Scarlet Monastery: Armory", "dungeon", 32, 42, 5),
    ("Scarlet Monastery: Cathedral", "dungeon", 35, 45, 5),
    ("Razorfen Downs", "dungeon", 37, 46, 5),
    ("Uldaman", "dungeon", 41, 51, 5),
    ("Zul'Farrak", "dungeon", 44, 54, 5),
    ("Maraudon", "dungeon", 46, 55, 5),
    ("Sunken Temple", "dungeon", 50, 56, 5),
    ("Blackrock Depths", "dungeon", 52, 60, 5),
    ("Lower Blackrock Spire", "dungeon", 55, 60, 5),
    ("Dire Maul: East", "dungeon", 55, 60, 5),
    ("Dire Maul: West", "dungeon", 57, 60, 5),
    ("Dire Maul: North", "dungeon", 57, 60, 5),
    ("Dire Maul: Tribute run", "dungeon", 58, 60, 5),
    ("Scholomance", "dungeon", 58, 60, 5),
    ("Stratholme: Live", "dungeon", 58, 60, 5),
    ("Stratholme: Undead", "dungeon", 58, 60, 5),
    ("Upper Blackrock Spire", "raid", 58, 60, 10),
    ("Zul'Gurub", "raid", 60, 60, 20),
    ("Ruins of Ahn'Qiraj", "raid", 60, 60, 20),
    ("Onyxia's Lair", "raid", 60, 60, 40),
    ("Molten Core", "raid", 60, 60, 40),
    ("Blackwing Lair", "raid", 60, 60, 40),
    ("Temple of Ahn'Qiraj", "raid", 60, 60, 40),
    ("Naxxramas", "raid", 60, 60, 40),
];

pub fn activities() -> Vec<Activity> {
    ACTIVITIES
        .iter()
        .map(|&(name, kind, min_level, max_level, size)| Activity {
            name: name.into(),
            kind: kind.into(),
            min_level,
            max_level,
            size,
        })
        .collect()
}

/// The listed activity with this name (any case), if it is one.
pub fn find_activity(name: &str) -> Option<Activity> {
    let name = name.trim();
    activities()
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
}

/// Usual tank / healer / DPS slots for a group of this size.
pub fn default_roles(size: i32) -> (i32, i32, i32) {
    match size {
        ..=5 => (1, 1, 3),
        6..=10 => (2, 3, 5),
        11..=20 => (3, 5, 12),
        _ => (5, 10, 25),
    }
}

// ---- Group finder ----------------------------------------------------------

/// A "looking for group" post, shown as a card in chat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LfgGroup {
    #[serde(with = "as_string")]
    pub message_id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    #[serde(with = "as_string")]
    pub leader_id: i64,
    pub activity: String,
    /// "dungeon", "raid" or "other".
    pub activity_kind: String,
    pub min_level: Option<i32>,
    pub max_level: Option<i32>,
    pub max_tanks: i32,
    pub max_healers: i32,
    pub max_dps: i32,
    pub starts_at: Option<DateTime<Utc>>,
    pub note: String,
    /// Which game it's for ("" = any).
    #[serde(default)]
    pub game: String,
    /// The leader closed it.
    pub closed: bool,
    /// Its time ran out.
    pub expired: bool,
    pub expires_at: DateTime<Utc>,
    /// Every slot is taken.
    pub full: bool,
    pub members: Vec<LfgMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LfgMember {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    /// "tank", "healer" or "dps".
    pub role: String,
    pub class: String,
    /// Character name ("" = not given).
    pub character: String,
    pub level: Option<i32>,
    pub joined_at: DateTime<Utc>,
}

/// `POST /api/channels/{id}/lfg`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateLfgRequest {
    /// A listed dungeon/raid name, or anything else ("Gold farming").
    pub activity: String,
    #[serde(default)]
    pub min_level: Option<i32>,
    #[serde(default)]
    pub max_level: Option<i32>,
    #[serde(default)]
    pub max_tanks: Option<i32>,
    #[serde(default)]
    pub max_healers: Option<i32>,
    #[serde(default)]
    pub max_dps: Option<i32>,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub note: String,
    /// How long the post stays up (default 2 hours, or until 2 hours after
    /// `starts_at`).
    #[serde(default)]
    pub expires_minutes: Option<i32>,
    /// The leader joins straight away in this role (optional).
    #[serde(default)]
    pub leader: Option<JoinLfgRequest>,
    /// "forever", "tbc", "era" or empty (any).
    #[serde(default)]
    pub game: String,
}

/// `PUT /api/channels/{id}/lfg/{message}/join`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JoinLfgRequest {
    pub role: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub character: String,
    #[serde(default)]
    pub level: Option<i32>,
}

// ---- Characters --------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct WowCharacter {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub user_id: i64,
    /// The (first) name.
    pub name: String,
    pub realm: String,
    pub class: String,
    pub level: i32,
    pub is_main: bool,
    pub professions: Vec<String>,
    pub armory_url: String,
    /// "manual" or "bnet".
    pub source: String,
    /// "forever", "tbc" or "era".
    #[serde(default)]
    pub game: String,
    /// WoW: Forever's second name ("" if not given).
    #[serde(default)]
    pub last_name: String,
    /// "First Last" for WoW: Forever, otherwise the name.
    #[serde(default)]
    pub full_name: String,
    /// WoW: Forever ruleset (Normal, PvP, Roleplay, Hardcore).
    #[serde(default)]
    pub ruleset: String,
    #[serde(default)]
    pub race: String,
}

/// Add or change a character (`POST /api/users/@me/characters`,
/// `PATCH /api/characters/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterRequest {
    pub name: String,
    #[serde(default)]
    pub realm: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub is_main: bool,
    #[serde(default)]
    pub professions: Vec<String>,
    #[serde(default)]
    pub armory_url: String,
    /// "forever" (default), "tbc" or "era".
    #[serde(default)]
    pub game: String,
    /// WoW: Forever's second name.
    #[serde(default)]
    pub last_name: String,
    /// WoW: Forever ruleset.
    #[serde(default)]
    pub ruleset: String,
    #[serde(default)]
    pub race: String,
}

/// `/item` and `/char`: what to look up.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WowLookupRequest {
    pub query: String,
}

/// `GET /api/bnet/status`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BnetStatus {
    /// The server has Battle.net keys set up.
    pub enabled: bool,
    pub linked: bool,
    #[serde(default)]
    pub battletag: Option<String>,
}

/// `GET /api/wow/info`: lists the client needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WowInfo {
    pub activities: Vec<Activity>,
    pub classes: Vec<String>,
    pub professions: Vec<String>,
    /// The games characters can be from, with their own options.
    #[serde(default)]
    pub games: Vec<GameInfo>,
    pub bnet_enabled: bool,
    /// Where item links point (e.g. https://www.wowhead.com/classic).
    pub wowhead_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_classes_only() {
        assert_eq!(CLASSES.len(), 9);
        for banned in ["Death Knight", "Demon Hunter", "Monk", "Evoker"] {
            assert!(class_name(banned).is_none());
        }
        assert_eq!(class_name("paladin"), Some("Paladin"));
    }

    #[test]
    fn games_differ() {
        assert_eq!(game_id(""), Some("forever"));
        assert_eq!(game_id("WoW: Forever"), Some("forever"));
        assert_eq!(game_id("TBC"), Some("tbc"));
        assert_eq!(game_id("Classic Era"), Some("era"));
        assert_eq!(game_id("Retail"), None);
        assert_eq!(max_level_for("tbc"), 70);
        assert_eq!(max_level_for("forever"), 60);
        assert!(professions_for("tbc").contains(&"Jewelcrafting"));
        assert!(!professions_for("forever").contains(&"Jewelcrafting"));
        assert!(races_for("tbc").contains(&"Blood Elf"));
        assert!(!races_for("era").contains(&"Draenei"));
        let g = games();
        assert!(g[0].two_names && g[0].rulesets.len() == 4 && !g[1].two_names);
    }

    #[test]
    fn activities_look_right() {
        let mc = find_activity("molten core").unwrap();
        assert_eq!((mc.kind.as_str(), mc.size), ("raid", 40));
        assert_eq!(default_roles(5), (1, 1, 3));
        assert!(ACTIVITIES.iter().all(|a| a.2 <= a.3 && a.3 <= MAX_LEVEL));
    }
}
