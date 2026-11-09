//! Soft-reserve exports that other tools read: softres.it's CSV and its
//! "Gargul export" (the string the Gargul addon imports in game with
//! `/gl softreserves` → Import), plus our own "Item — Player, Player" list.
//!
//! The Gargul string is base64( zlib( JSON ) ), with the JSON shaped and
//! escaped exactly like softres.it's (PHP's `json_encode`: `\/` for slashes,
//! `\uXXXX` for anything that isn't ASCII), so the addon can't tell them apart.

use std::collections::HashMap;
use std::io::Write;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use base64::Engine;
use chrono::{DateTime, Utc};
use outhouse_common::models::{EventSignup, RaidEvent};
use serde::{Deserialize, Serialize};

use super::events::{build, load};
use super::loot::{csv_field, person, require_manager, signup_of, table_items};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const CSV_HEADER: &str =
    "Item Name,Item ID,From,Raider Name,Raider Class,Raider Spec,Raider Note,Extra Reserves,Date\n";

#[derive(Deserialize, Default)]
pub struct ExportQuery {
    /// "first": WoW: Forever characters by their first name only
    /// (default: "First Last").
    #[serde(default)]
    names: Option<String>,
}

impl ExportQuery {
    fn first_only(&self) -> bool {
        self.names.as_deref() == Some("first")
    }
}

/// The name a raider goes by in game: their character's (optionally just
/// the first of a WoW: Forever "First Last"), else their display name.
fn raider_name(s: Option<&EventSignup>, fallback: &str, first_only: bool) -> String {
    if first_only {
        if let Some(c) = s.and_then(|s| s.character.as_ref()) {
            if !c.name.is_empty() {
                return c.name.clone();
            }
        }
    }
    person(s, fallback)
}

fn raider_class(s: Option<&EventSignup>) -> String {
    let Some(s) = s else {
        return String::new();
    };
    if !s.class.is_empty() {
        return s.class.clone();
    }
    s.character
        .as_ref()
        .map(|c| c.class.clone())
        .unwrap_or_default()
}

/// One reserve, as softres.it lists it.
struct Row {
    item_name: String,
    item_id: i32,
    from: String,
    raider: String,
    class: String,
    spec: String,
    note: String,
    user_id: i64,
    at: DateTime<Utc>,
}

/// Each reserve with its raider (in reserve order).
fn rows(ev: &RaidEvent, bosses: &HashMap<i32, String>, first_only: bool) -> Vec<Row> {
    ev.reserves
        .iter()
        .map(|r| {
            let s = signup_of(&ev.signups, r.user_id);
            let fallback = s.map(|s| s.display_name.clone()).unwrap_or_default();
            Row {
                item_name: r.item_name.clone(),
                item_id: r.item_id,
                from: bosses.get(&r.item_id).cloned().unwrap_or_default(),
                raider: raider_name(s, &fallback, first_only),
                class: raider_class(s),
                spec: s.map(|s| s.spec.clone()).unwrap_or_default(),
                note: s.map(|s| s.note.trim().to_string()).unwrap_or_default(),
                user_id: r.user_id,
                at: r.reserved_at,
            }
        })
        .collect()
}

/// "Danité" → "Danité"; "shaman" → "Shaman" (classes and specs as softres.it
/// writes them).
fn capitalised(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut start = true;
    for c in raw.trim().chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = c == ' ' || c == '-';
    }
    out
}

/// softres.it's CSV: one row per reserve, by raider name.
fn softres_csv(mut rows: Vec<Row>) -> String {
    rows.sort_by(|a, b| {
        a.raider
            .to_lowercase()
            .cmp(&b.raider.to_lowercase())
            .then(a.at.cmp(&b.at))
            .then(a.item_id.cmp(&b.item_id))
    });
    let mut csv = String::from(CSV_HEADER);
    for r in rows {
        csv.push_str(
            &[
                csv_field(&r.item_name),
                r.item_id.to_string(),
                csv_field(&r.from),
                csv_field(&r.raider),
                csv_field(&capitalised(&r.class)),
                csv_field(&capitalised(&r.spec)),
                csv_field(&r.note),
                "0".into(),
                r.at.format("%Y-%m-%d %H:%M:%S").to_string(),
            ]
            .join(","),
        );
        csv.push('\n');
    }
    csv
}

// ---- Gargul -------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GargulMeta {
    pub id: String,
    pub url: String,
    pub instances: Vec<String>,
    pub hidden: bool,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(rename = "raidStartsAt")]
    pub raid_starts_at: i64,
    #[serde(rename = "lockedAt")]
    pub locked_at: Option<i64>,
    #[serde(rename = "discordUrl")]
    pub discord_url: String,
    pub note: Option<String>,
    pub bonusrolls_consumed_on_use: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GargulItem {
    pub id: i32,
    pub note: String,
    pub order: i32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GargulRaider {
    pub name: String,
    pub class: String,
    pub note: Option<String>,
    pub items: Vec<GargulItem>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GargulExport {
    pub metadata: GargulMeta,
    pub softreserves: Vec<GargulRaider>,
    pub hardreserves: Vec<serde_json::Value>,
    pub bonusrolls: Vec<serde_json::Value>,
}

/// JSON the way PHP's `json_encode` writes it: `/` as `\/` and anything
/// past ASCII as `\uXXXX` (UTF-16, lower-case hex). Both only ever appear
/// inside strings, so the whole document can be rewritten in one pass.
fn php_json<T: Serialize>(value: &T) -> String {
    let raw = serde_json::to_string(value).unwrap_or_default();
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        match c {
            '/' => out.push_str("\\/"),
            c if (c as u32) < 0x80 => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out
}

/// base64( zlib( JSON ) ), as softres.it's "Gargul export".
fn gargul_string(json: &str) -> String {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing to a Vec can't fail.
    let _ = z.write_all(json.as_bytes());
    let bytes = z.finish().unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A short, stable ID for a raid (base62 of its ID, eight characters), like
/// softres.it's "neg56QZb".
fn short_id(event_id: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut n = event_id as u64;
    let mut out = Vec::with_capacity(8);
    for _ in 0..8 {
        out.push(DIGITS[(n % 62) as usize]);
        n /= 62;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// softres.it's name for a raid. Black Temple's ("blacktemple") is
/// confirmed from a real export; the rest follow the same pattern (Gargul
/// doesn't need them to import). Unlisted raids: their name, letters and
/// digits only.
fn instance_slug(key: &str) -> Option<String> {
    let slug = match key {
        "mc" => "moltencore",
        "ony" => "onyxia",
        "bwl" => "bwl",
        "zg" => "zg",
        "aq20" => "aq20",
        "aq40" => "aq40",
        "naxx" => "naxxramas",
        "kara" => "karazhan",
        "gruul" | "mag" => "gruulmag",
        "ssc" => "serpentshrine",
        "tk" => "tempestkeep",
        "hyjal" => "hyjal",
        "bt" => "blacktemple",
        "za" => "zulaman",
        "swp" => "sunwell",
        other => {
            let s: String = other
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .take(40)
                .collect();
            return (!s.is_empty()).then_some(s);
        }
    };
    Some(slug.to_string())
}

/// When reserves locked (if they have).
fn locked_at(ev: &RaidEvent, now: DateTime<Utc>) -> Option<i64> {
    if !ev.reserves_locked {
        return None;
    }
    let end = ev.starts_at + chrono::Duration::minutes(ev.duration_minutes as i64);
    let auto = match ev.reserve_lock.as_str() {
        "manual" => None,
        "start" => Some(ev.starts_at),
        _ => Some(ev.signups_close_at.unwrap_or(ev.starts_at)),
    };
    let at = match auto {
        Some(t) if t <= now => t,
        _ if end <= now => end,
        _ => now,
    };
    Some(at.timestamp())
}

/// Raiders (by name), each with their reserves in the order they made them.
fn gargul_raiders(rows: &[Row]) -> Vec<GargulRaider> {
    let mut out: Vec<(i64, GargulRaider)> = Vec::new();
    for r in rows {
        let i = match out.iter().position(|(u, _)| *u == r.user_id) {
            Some(i) => i,
            None => {
                out.push((
                    r.user_id,
                    GargulRaider {
                        name: r.raider.clone(),
                        class: r.class.to_lowercase().replace(' ', ""),
                        note: Some(r.note.clone()).filter(|n| !n.is_empty()),
                        items: Vec::new(),
                    },
                ));
                out.len() - 1
            }
        };
        let items = &mut out[i].1.items;
        let order = items.len() as i32;
        items.push(GargulItem {
            id: r.item_id,
            note: String::new(),
            order,
        });
    }
    let mut raiders: Vec<GargulRaider> = out.into_iter().map(|(_, r)| r).collect();
    raiders.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.name.cmp(&b.name))
    });
    raiders
}

fn gargul_export(ev: &RaidEvent, rows: &[Row], url: String) -> GargulExport {
    let created = (outhouse_common::id::created_at_ms(ev.id) / 1000) as i64;
    let updated = rows
        .iter()
        .map(|r| r.at.timestamp())
        .max()
        .unwrap_or(created)
        .max(created);
    let note = ev
        .description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect());
    GargulExport {
        metadata: GargulMeta {
            id: short_id(ev.id),
            url,
            instances: instance_slug(&ev.loot_instance).into_iter().collect(),
            hidden: false,
            created_at: created,
            updated_at: updated,
            raid_starts_at: ev.starts_at.timestamp(),
            locked_at: locked_at(ev, Utc::now()),
            discord_url: String::new(),
            note,
            bonusrolls_consumed_on_use: false,
        },
        softreserves: gargul_raiders(rows),
        hardreserves: Vec::new(),
        bonusrolls: Vec::new(),
    }
}

// ---- Endpoints --------------------------------------------------------------------

/// The raid with its reserves, and who drops what on its loot list
/// (raid leaders only).
async fn raid_for_export(
    st: &AppState,
    event_id: i64,
    user_id: i64,
) -> AppResult<(RaidEvent, HashMap<i32, String>)> {
    let (row, access) = load(st, event_id, user_id).await?;
    require_manager(st, &row, &access).await?;
    let bosses: HashMap<i32, String> = table_items(st, &row)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.boss.is_empty())
        .map(|i| (i.item_id, i.boss))
        .collect();
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    Ok((ev, bosses))
}

#[derive(Serialize)]
pub struct ReserveExport {
    /// softres.it's CSV (see [`CSV_HEADER`]), one row per reserve.
    pub csv: String,
    /// "Item — Player1, Player2", one item per line.
    pub text: String,
}

/// `GET /api/events/{id}/reserves/export[?names=first]` (raid leaders).
pub async fn export(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Query(q): Query<ExportQuery>,
) -> AppResult<Json<ReserveExport>> {
    let (ev, bosses) = raid_for_export(&st, event_id, user.id).await?;
    let list = rows(&ev, &bosses, q.first_only());
    let mut by_item: Vec<(i32, String, Vec<String>)> = Vec::new();
    for r in &list {
        match by_item.iter_mut().find(|i| i.0 == r.item_id) {
            Some(i) => i.2.push(r.raider.clone()),
            None => by_item.push((r.item_id, r.item_name.clone(), vec![r.raider.clone()])),
        }
    }
    by_item.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    let text = by_item
        .iter()
        .map(|(_, item, names)| format!("{item} — {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Json(ReserveExport {
        csv: softres_csv(list),
        text,
    }))
}

#[derive(Serialize)]
pub struct GargulReply {
    /// Paste into Gargul's "Import soft-reserves".
    pub text: String,
    pub raiders: usize,
    pub reserves: usize,
}

/// `GET /api/events/{id}/softres/gargul[?names=first]` (raid leaders).
pub async fn gargul(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Query(q): Query<ExportQuery>,
    headers: HeaderMap,
) -> AppResult<Json<GargulReply>> {
    let (ev, bosses) = raid_for_export(&st, event_id, user.id).await?;
    let list = rows(&ev, &bosses, q.first_only());
    let url = format!(
        "{}/app?guild={}&channel={}&event={}",
        super::invites::public_base(&st, &headers),
        ev.guild_id,
        ev.channel_id,
        ev.id
    );
    let export = gargul_export(&ev, &list, url);
    Ok(Json(GargulReply {
        text: gargul_string(&php_json(&export)),
        raiders: export.softreserves.len(),
        reserves: list.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// softres.it's real "Gargul export" for a Black Temple raid.
    const EXAMPLE: &str = include_str!("../../testdata/softres_gargul_example.txt");

    fn decode(text: &str) -> String {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .expect("base64");
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(&bytes[..])
            .read_to_string(&mut out)
            .expect("zlib");
        out
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn same_json_as_softres() {
        let original = decode(EXAMPLE);
        let parsed: GargulExport = serde_json::from_str(&original).expect("softres.it's shape");
        assert_eq!(parsed.metadata.instances, vec!["blacktemple"]);
        assert!(parsed.softreserves.iter().any(|r| r.name == "Danité"));
        // Rebuilt from the same reserves, ours is byte for byte the same.
        let rows: Vec<Row> = parsed
            .softreserves
            .iter()
            .enumerate()
            .flat_map(|(n, r)| {
                r.items.iter().map(move |i| Row {
                    item_name: String::new(),
                    item_id: i.id,
                    from: String::new(),
                    raider: r.name.clone(),
                    class: capitalised(&r.class),
                    spec: String::new(),
                    note: r.note.clone().unwrap_or_default(),
                    user_id: 1000 - n as i64,
                    at: at("2026-11-08T00:24:36Z"),
                })
            })
            .rev()
            .collect();
        let rebuilt = GargulExport {
            metadata: GargulMeta {
                id: "neg56QZb".into(),
                url: "https://softres.it/raid/neg56QZb".into(),
                instances: vec![instance_slug("bt").unwrap()],
                hidden: false,
                created_at: 1793674167,
                updated_at: 1794130256,
                raid_starts_at: 1794130200,
                locked_at: None,
                discord_url: String::new(),
                note: None,
                bonusrolls_consumed_on_use: false,
            },
            softreserves: gargul_raiders(&rows),
            hardreserves: vec![],
            bonusrolls: vec![],
        };
        let ours = php_json(&rebuilt);
        assert_eq!(ours, original);
        // And our string decodes back to it.
        let text = gargul_string(&ours);
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&text)
            .unwrap()
            .starts_with(&[0x78, 0xda]));
        assert_eq!(decode(&text), ours);
        let back: GargulExport = serde_json::from_str(&decode(&text)).unwrap();
        assert_eq!(back, parsed);
    }

    #[test]
    fn escapes_like_php() {
        assert_eq!(
            php_json(&"https://x.y/a Danité 🐉"),
            "\"https:\\/\\/x.y\\/a Danit\\u00e9 \\ud83d\\udc09\""
        );
    }

    #[test]
    fn raiders_group_their_items() {
        let row = |user: i64, name: &str, item: i32| Row {
            item_name: "x".into(),
            item_id: item,
            from: String::new(),
            raider: name.into(),
            class: "Warrior".into(),
            spec: "Arms".into(),
            note: if user == 1 {
                "Alt of Bob".into()
            } else {
                String::new()
            },
            user_id: user,
            at: at("2026-11-03T02:51:16Z"),
        };
        let r = gargul_raiders(&[row(2, "Zed", 5), row(1, "anna", 7), row(2, "Zed", 6)]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "anna");
        assert_eq!(r[0].class, "warrior");
        assert_eq!(r[0].note.as_deref(), Some("Alt of Bob"));
        assert_eq!(r[1].note, None);
        assert_eq!(
            r[1].items,
            vec![
                GargulItem {
                    id: 5,
                    note: String::new(),
                    order: 0
                },
                GargulItem {
                    id: 6,
                    note: String::new(),
                    order: 1
                }
            ]
        );
    }

    #[test]
    fn csv_like_softres() {
        let row = |name: &str, item: &str, id: i32, from: &str, note: &str| Row {
            item_name: item.into(),
            item_id: id,
            from: from.into(),
            raider: name.into(),
            class: "shaman".into(),
            spec: "elemental".into(),
            note: note.into(),
            user_id: 1,
            at: at("2026-11-03T02:51:16Z"),
        };
        let csv = softres_csv(vec![
            row(
                "Shambulàncè",
                "Madness of the Betrayer",
                32505,
                "The Illidari Council",
                "say \"hi\"",
            ),
            row(
                "Avizandum",
                "Zhar'doom, Greatstaff of the Devourer",
                32374,
                "Illidan Stormrage",
                "",
            ),
        ]);
        assert_eq!(
            csv,
            format!(
                "{CSV_HEADER}\"Zhar'doom, Greatstaff of the Devourer\",32374,Illidan Stormrage,Avizandum,Shaman,Elemental,,0,2026-11-03 02:51:16\n\
                 Madness of the Betrayer,32505,The Illidari Council,Shambulàncè,Shaman,Elemental,\"say \"\"hi\"\"\",0,2026-11-03 02:51:16\n"
            )
        );
    }

    #[test]
    fn ids_and_slugs() {
        assert_eq!(short_id(0), "00000000");
        assert_eq!(short_id(61), "0000000Z");
        assert_eq!(short_id(1_234_567_890_123_456_789).len(), 8);
        assert_eq!(instance_slug("bt").as_deref(), Some("blacktemple"));
        assert_eq!(instance_slug("mag").as_deref(), Some("gruulmag"));
        assert_eq!(
            instance_slug("scarlet raid").as_deref(),
            Some("scarletraid")
        );
        assert_eq!(instance_slug(""), None);
        assert_eq!(capitalised("feral druid"), "Feral Druid");
    }
}
