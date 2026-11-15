//! Loot history from Gargul, and the outhouse's loot history views.
//!
//! Raid leaders paste Gargul's "awarded loot" export (a CSV with a header
//! line; Gargul lets you change its columns, so they're found by name). Each
//! row becomes a loot_awards entry: matched to a raid (the raid it was
//! pasted on, or by date and who signed up) and to a member (by character
//! name, without case or accents) where it can be. `_disenchanted` rows are
//! kept as disenchanted, unmatched names are kept as they were written, and
//! Gargul's award IDs keep a second paste from adding anything twice. Each
//! paste is one import its importer can undo for 24 hours.
//!
//! A soft reserve that's won is marked won; the outhouse picks (Outhouse
//! settings → Raids) whether it stays, locks the item for that character in
//! later raids of the same instance (the default), or is removed.
//!
//! Also here: the outhouse's loot history with filters and sorting, the
//! per-character summary, the CSV export, and each member's loot by
//! character (their full profile).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use outhouse_common::id::{as_string, option_as_string};
use outhouse_common::permissions as p;
use outhouse_common::raidplan::{self, LootAward};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use super::events::{broadcast, load};
use super::loot::{
    csv_field, fill_bosses, fill_branches, outhouse_loot_manager, require_manager, AwardRow,
    AWARD_SELECT,
};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{drop_age_gated, member_ctx, visible_text_channels, MemberCtx};
use crate::state::AppState;

/// Biggest paste (about 15,000 rows of Gargul's default columns).
const MAX_TEXT: usize = 1_000_000;
const MAX_ROWS: usize = 5000;
/// How long the importer can undo an import.
const UNDO_HOURS: i64 = 24;

// ---- Names -------------------------------------------------------------------

/// What character names are matched on: the name without a "-Realm",
/// accents or case ("Shamièè" → "shamiee"), spaces tidied.
pub(crate) fn char_key(raw: &str) -> String {
    let name = raw.trim().split('-').next().unwrap_or("");
    let folded: String = name
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .nfc()
        .collect();
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A name with this key: its first name, or "First Last" (WoW: Forever).
pub(crate) fn name_is(key: &str, name: &str, last: &str) -> bool {
    if key.is_empty() || name.trim().is_empty() {
        return false;
    }
    key == char_key(name) || (!last.trim().is_empty() && key == char_key(&format!("{name} {last}")))
}

/// An award (by its character key) was won by this sign-up's character.
/// Unknown on either side counts as the same.
pub(crate) fn same_character(award_key: &str, name: &str, last: &str) -> bool {
    award_key.is_empty() || name.trim().is_empty() || name_is(award_key, name, last)
}

/// Item IDs from The Burning Crusade (for Wowhead links when there's no
/// raid to tell).
pub(crate) fn looks_tbc(item_id: i32) -> bool {
    (24_000..40_000).contains(&item_id)
}

fn is_disenchanted(key: &str) -> bool {
    matches!(
        key,
        "_disenchanted" | "disenchanted" | "_de" | "_disenchant"
    )
}

// ---- Reading Gargul's export -----------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct GRow {
    line: usize,
    gargul_id: String,
    date: NaiveDate,
    time: Option<NaiveTime>,
    character: String,
    key: String,
    disenchanted: bool,
    item_id: i32,
    offspec: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Parsed {
    rows: Vec<GRow>,
    errors: Vec<LineError>,
}

/// CSV records (RFC 4180: quotes, doubled quotes, line breaks in quotes),
/// each with the line it starts on. Blank lines are dropped.
fn csv_records(text: &str, delim: char) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut field_started = false;
    let mut line = 1;
    let mut start_line = 1;
    let mut chars = text.chars().peekable();
    let finish = |record: &mut Vec<String>, out: &mut Vec<(usize, Vec<String>)>, at: usize| {
        if !(record.len() == 1 && record[0].trim().is_empty()) {
            out.push((at, std::mem::take(record)));
        } else {
            record.clear();
        }
    };
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                '\r' => {}
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if !field_started || field.trim().is_empty() => {
                field.clear();
                quoted = true;
                field_started = true;
            }
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                finish(&mut record, &mut out, start_line);
                field_started = false;
                line += 1;
                start_line = line;
            }
            c if c == delim => {
                record.push(std::mem::take(&mut field));
                field_started = false;
            }
            _ => {
                field.push(c);
                field_started = true;
            }
        }
    }
    if field_started || !field.is_empty() || !record.is_empty() {
        record.push(field);
        finish(&mut record, &mut out, start_line);
    }
    out
}

/// A header cell, for matching: lower case letters and digits only.
fn header_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn find_col(headers: &[String], names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|n| headers.iter().position(|h| h == n))
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "x" | "os" | "offspec"
    )
}

/// An item ID: a number, or the one in an item link (`|Hitem:32240:…`,
/// `item=32240`).
fn parse_item(raw: &str) -> Option<i32> {
    let t = raw.trim();
    let id = if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        t.parse().ok()
    } else {
        let lower = t.to_lowercase();
        ["item:", "item="].iter().find_map(|pat| {
            let at = lower.find(pat)? + pat.len();
            let digits: String = lower[at..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse().ok()
        })
    };
    id.filter(|i| (1..=10_000_000).contains(i))
}

/// A date, or a date and time, in the formats Gargul can be set to (and a
/// few more): ISO dates and times, day/month/year, Unix time.
fn parse_when(raw: &str) -> Option<(NaiveDate, Option<NaiveTime>)> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let ok = |d: NaiveDate| (2004..=2100).contains(&chrono::Datelike::year(&d));
    if (9..=13).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit()) {
        let n: i64 = t.parse().ok()?;
        let secs = if t.len() > 11 { n / 1000 } else { n };
        let dt = DateTime::<Utc>::from_timestamp(secs, 0)?;
        return ok(dt.date_naive()).then(|| (dt.date_naive(), Some(dt.time())));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
        let n = dt.naive_local();
        return ok(n.date()).then(|| (n.date(), Some(n.time())));
    }
    let t = t.trim_end_matches('Z').trim();
    const WITH_TIME: [&str; 14] = [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
        "%d.%m.%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
    ];
    for f in WITH_TIME {
        if let Ok(n) = NaiveDateTime::parse_from_str(t, f) {
            return ok(n.date()).then(|| (n.date(), Some(n.time())));
        }
    }
    const DATES: [&str; 6] = [
        "%Y-%m-%d", "%Y/%m/%d", "%d/%m/%Y", "%m/%d/%Y", "%d.%m.%Y", "%d-%m-%Y",
    ];
    for f in DATES {
        if let Ok(d) = NaiveDate::parse_from_str(t, f) {
            return ok(d).then_some((d, None));
        }
    }
    None
}

/// Reads Gargul's awarded-loot export. Bad rows are listed (by line) and
/// left out; a paste that isn't an export at all is an error.
pub(crate) fn parse_gargul(text: &str) -> Result<Parsed, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Err("Paste Gargul's awarded loot export first.".into());
    }
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let delim = [',', ';', '\t']
        .into_iter()
        .max_by_key(|d| first_line.matches(*d).count())
        .filter(|d| first_line.contains(*d))
        .unwrap_or(',');
    let mut records = csv_records(text, delim);
    if records.is_empty() {
        return Err("Paste Gargul's awarded loot export first.".into());
    }
    let headers: Vec<String> = records[0].1.iter().map(|h| header_name(h)).collect();
    let mut date_col = find_col(
        &headers,
        &[
            "datetime",
            "date",
            "awardedon",
            "awardedat",
            "timestamp",
            "when",
            "day",
            "time",
        ],
    );
    let mut char_col = find_col(
        &headers,
        &[
            "character",
            "charactername",
            "player",
            "playername",
            "winner",
            "awardedto",
            "receiver",
            "recipient",
            "name",
        ],
    );
    let mut item_col = find_col(&headers, &["itemid", "item", "itemlink", "link"]);
    let mut os_col = find_col(&headers, &["offspec", "os", "isoffspec"]);
    let mut id_col = find_col(
        &headers,
        &["id", "awardid", "gargulid", "checksum", "uuid", "guid"],
    );
    let de_col = find_col(&headers, &["disenchanted", "de"]);
    if char_col.is_none() || item_col.is_none() {
        // No header line: Gargul's default columns, if the first row looks like one.
        let looks_like_data = records[0].1.len() >= 3
            && parse_when(&records[0].1[0]).is_some()
            && parse_item(&records[0].1[2]).is_some();
        if !looks_like_data {
            return Err("That doesn't look like Gargul's awarded loot export: it needs a header line with (at least) character and itemID columns.".into());
        }
        (date_col, char_col, item_col, os_col, id_col) =
            (Some(0), Some(1), Some(2), Some(3), Some(4));
    } else {
        records.remove(0);
    }
    if records.len() > MAX_ROWS {
        return Err(format!(
            "That's {} rows; up to {MAX_ROWS} at a time, please. Gargul can export a date range.",
            records.len()
        ));
    }
    let (char_col, item_col) = (char_col.unwrap_or(1), item_col.unwrap_or(2));
    let cell = |r: &[String], c: Option<usize>| -> String {
        c.and_then(|c| r.get(c))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut parsed = Parsed::default();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut auto_n: HashMap<String, usize> = HashMap::new();
    for (line, rec) in records {
        let character: String = cell(&rec, Some(char_col)).nfc().collect();
        let err = |m: &str| LineError {
            line,
            message: m.to_string(),
        };
        if character.is_empty() {
            parsed.errors.push(err("No character name."));
            continue;
        }
        let Some(item_id) = parse_item(&cell(&rec, Some(item_col))) else {
            parsed.errors.push(err("No item ID."));
            continue;
        };
        let when = cell(&rec, date_col);
        let Some((date, time)) = parse_when(&when) else {
            parsed.errors.push(err(if when.is_empty() {
                "No date."
            } else {
                "Couldn't read the date."
            }));
            continue;
        };
        let key = char_key(&character);
        let disenchanted =
            is_disenchanted(&key) || de_col.is_some_and(|c| parse_bool(&cell(&rec, Some(c))));
        let offspec = !disenchanted && os_col.is_some_and(|c| parse_bool(&cell(&rec, Some(c))));
        let given: String = cell(&rec, id_col).chars().take(100).collect();
        let gargul_id = if given.is_empty() {
            // No ID column: the same row pasted again gets the same ID.
            let base = format!("auto:{date}:{key}:{item_id}:{}", u8::from(offspec));
            let n = auto_n.entry(base.clone()).or_default();
            *n += 1;
            format!("{base}:{n}")
        } else {
            given
        };
        if let Some(first) = seen.get(&gargul_id) {
            parsed.errors.push(err(&format!(
                "Same award ID as line {first}, so it's left out."
            )));
            continue;
        }
        seen.insert(gargul_id.clone(), line);
        parsed.rows.push(GRow {
            line,
            gargul_id,
            date,
            time,
            key: if disenchanted { String::new() } else { key },
            character,
            disenchanted,
            item_id,
            offspec,
        });
    }
    if parsed.rows.is_empty() && !parsed.errors.is_empty() {
        let first = &parsed.errors[0];
        return Err(format!(
            "Nothing could be read (line {}: {})",
            first.line, first.message
        ));
    }
    Ok(parsed)
}

// ---- Raids, sign-ups and characters to match against -------------------------------

#[derive(sqlx::FromRow)]
pub(crate) struct RaidRow {
    pub(crate) id: i64,
    pub(crate) channel_id: i64,
    pub(crate) title: String,
    pub(crate) starts_at: DateTime<Utc>,
    pub(crate) duration_minutes: i32,
    pub(crate) loot_instance: String,
    pub(crate) game: String,
    series_tz: Option<String>,
}

pub(crate) struct Raid {
    pub(crate) row: RaidRow,
    pub(crate) tz: String,
    pub(crate) local_start: NaiveDateTime,
    pub(crate) signups: Vec<Sign>,
}

impl Raid {
    pub(crate) fn local_date(&self) -> NaiveDate {
        self.local_start.date()
    }
    pub(crate) fn near(&self, d: NaiveDate) -> bool {
        (self.local_date() - d).num_days().abs() <= 1
    }
    /// Within the raid's evening (an hour before to three hours after).
    pub(crate) fn during(&self, d: NaiveDate, t: NaiveTime) -> bool {
        let at = d.and_time(t);
        let from = self.local_start - chrono::Duration::hours(1);
        let to = self.local_start
            + chrono::Duration::minutes(self.row.duration_minutes.max(60) as i64)
            + chrono::Duration::hours(3);
        at >= from && at <= to
    }
}

#[derive(sqlx::FromRow, Clone)]
pub(crate) struct Sign {
    pub(crate) event_id: i64,
    pub(crate) user_id: i64,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) class: String,
    pub(crate) character_name: String,
    pub(crate) character_last_name: String,
    pub(crate) character_class: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct MemberChar {
    user_id: i64,
    name: String,
    last_name: String,
    class: String,
    display_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Matched {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    pub class: String,
    /// "signup" (their sign-up in that raid) or "character" (a character on
    /// their profile).
    pub via: &'static str,
}

pub(crate) fn match_in_raid(raid: &Raid, key: &str) -> Option<Matched> {
    if key.is_empty() {
        return None;
    }
    let mut people: Vec<&Sign> = raid.signups.iter().collect();
    // People who came first.
    people.sort_by_key(|s| s.status == "no");
    let found = people
        .iter()
        .find(|s| name_is(key, &s.character_name, &s.character_last_name))
        .or_else(|| {
            people
                .iter()
                .find(|s| s.character_name.is_empty() && char_key(&s.display_name) == key)
        })?;
    Some(Matched {
        user_id: found.user_id,
        display_name: found.display_name.clone(),
        class: if found.character_class.is_empty() {
            found.class.clone()
        } else {
            found.character_class.clone()
        },
        via: "signup",
    })
}

pub(crate) fn match_member(chars: &[MemberChar], key: &str) -> Option<Matched> {
    if key.is_empty() {
        return None;
    }
    let hits: Vec<&MemberChar> = chars
        .iter()
        .filter(|c| name_is(key, &c.name, &c.last_name))
        .collect();
    let first = hits.first()?;
    // Two members with a character of that name: can't tell.
    if hits.iter().any(|h| h.user_id != first.user_id) {
        return None;
    }
    Some(Matched {
        user_id: first.user_id,
        display_name: first.display_name.clone(),
        class: first.class.clone(),
        via: "character",
    })
}

struct Ctx {
    raids: Vec<Raid>,
    chars: Vec<MemberChar>,
    default_tz: String,
    reserves: HashSet<(i64, i64, i32)>,
    existing: HashSet<String>,
    names: HashMap<i32, (String, i32)>,
}

impl Ctx {
    fn raid(&self, id: i64) -> Option<&Raid> {
        self.raids.iter().find(|r| r.row.id == id)
    }
    fn who(&self, raid: Option<&Raid>, row: &GRow) -> Option<Matched> {
        if row.disenchanted {
            return None;
        }
        raid.and_then(|r| match_in_raid(r, &row.key))
            .or_else(|| match_member(&self.chars, &row.key))
    }
}

/// Item names and qualities the server already knows (lists, tooltips,
/// reserves, earlier loot).
async fn known_items(
    db: &PgPool,
    guild_id: i64,
    ids: &[i32],
) -> AppResult<HashMap<i32, (String, i32)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i32, String, i32)> = sqlx::query_as(
        "SELECT DISTINCT ON (item_id) item_id, name, quality FROM (
             SELECT item_id, name, quality, 1 AS pref FROM wow_items
              WHERE item_id = ANY($1) AND name <> ''
             UNION ALL
             SELECT item_id, name, quality, 2 FROM wowhead_drops WHERE item_id = ANY($1)
             UNION ALL
             SELECT item_id, name, quality, 3 FROM loot_items
              WHERE guild_id = $2 AND item_id = ANY($1)
             UNION ALL
             SELECT item_id, item_name, item_quality, 4 FROM loot_reserves WHERE item_id = ANY($1)
             UNION ALL
             SELECT item_id, item_name, item_quality, 5 FROM loot_awards
              WHERE item_id = ANY($1) AND item_name <> 'Item ' || item_id::TEXT
         ) x ORDER BY item_id, pref",
    )
    .bind(ids)
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i32, (String, i32)> =
        rows.into_iter().map(|(i, n, q)| (i, (n, q))).collect();
    for s in raidplan::SEED_ITEMS {
        if ids.contains(&s.1) {
            map.entry(s.1).or_insert((s.2.to_string(), s.3));
        }
    }
    Ok(map)
}

/// Looks unknown items up on Wowhead in the background (cached for next
/// time), and names imported loot that only had an ID.
fn fetch_names_later(st: &AppState, ids: Vec<i32>) {
    if ids.is_empty() {
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        for id in ids.into_iter().take(200) {
            let env = if looks_tbc(id) {
                st.config.wowhead_data_env_tbc.clone()
            } else {
                st.config.wowhead_data_env.clone()
            };
            let found = tokio::time::timeout(
                Duration::from_secs(10),
                crate::wowhead::item_by_id_env(&st, id, &env),
            )
            .await;
            if let Ok(Ok(Some(item))) = found {
                if !item.name.is_empty() {
                    let _ = sqlx::query(
                        "UPDATE loot_awards SET item_name = $2, item_quality = $3
                         WHERE item_id = $1 AND item_name = 'Item ' || $1::TEXT",
                    )
                    .bind(id)
                    .bind(&item.name)
                    .bind(item.quality)
                    .execute(&st.db)
                    .await;
                }
            }
        }
    });
}

/// Everything rows are matched against: the raids (one, or the outhouse's
/// around the rows' dates in channels the importer can see), their sign-ups
/// and reserves, members' characters, what's already imported.
async fn context(
    st: &AppState,
    guild_id: i64,
    channels: &[i64],
    only: Option<i64>,
    rows: &[GRow],
) -> AppResult<Ctx> {
    let db = &st.db;
    let first = rows.iter().map(|r| r.date).min();
    let last = rows.iter().map(|r| r.date).max();
    let (from, to) = match (first, last) {
        (Some(a), Some(b)) => (
            (a - chrono::Duration::days(2))
                .and_time(NaiveTime::MIN)
                .and_utc(),
            (b + chrono::Duration::days(3))
                .and_time(NaiveTime::MIN)
                .and_utc(),
        ),
        _ => (Utc::now(), Utc::now()),
    };
    let (raids, default_tz) = raids_between(st, guild_id, channels, only, from, to).await?;
    let ids: Vec<i64> = raids.iter().map(|r| r.row.id).collect();
    let chars = member_chars(db, guild_id).await?;
    let reserves: HashSet<(i64, i64, i32)> = sqlx::query_as::<_, (i64, i64, i32)>(
        "SELECT event_id, user_id, item_id FROM loot_reserves WHERE event_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    let gids: Vec<String> = rows.iter().map(|r| r.gargul_id.clone()).collect();
    let existing: HashSet<String> = sqlx::query_scalar(
        "SELECT gargul_id FROM loot_awards WHERE guild_id = $1 AND gargul_id = ANY($2)",
    )
    .bind(guild_id)
    .bind(&gids)
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    let mut item_ids: Vec<i32> = rows.iter().map(|r| r.item_id).collect();
    item_ids.sort_unstable();
    item_ids.dedup();
    let names = known_items(db, guild_id, &item_ids).await?;
    Ok(Ctx {
        raids,
        chars,
        default_tz,
        reserves,
        existing,
        names,
    })
}

/// The outhouse's raids in these channels starting between `from` and `to`
/// (or just raid `only`), each with its local start (its series' time zone,
/// else its game's realm zone, else the outhouse's) and its sign-ups; and
/// the outhouse's own time zone.
pub(crate) async fn raids_between(
    st: &AppState,
    guild_id: i64,
    channels: &[i64],
    only: Option<i64>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<(Vec<Raid>, String)> {
    let db = &st.db;
    let raid_rows = sqlx::query_as::<_, RaidRow>(
        "SELECT e.id, e.channel_id, e.title, e.starts_at, e.duration_minutes, e.loot_instance,
                e.game, es.time_zone AS series_tz
         FROM events e LEFT JOIN event_series es ON es.id = e.series_id
         WHERE e.guild_id = $1 AND e.channel_id = ANY($2)
           AND (($3::BIGINT IS NOT NULL AND e.id = $3)
                OR ($3::BIGINT IS NULL AND e.starts_at >= $4 AND e.starts_at < $5))
         ORDER BY e.starts_at, e.id",
    )
    .bind(guild_id)
    .bind(channels)
    .bind(only)
    .bind(from)
    .bind(to)
    .fetch_all(db)
    .await?;
    // Each raid's day in its own time zone (its series', else the realm's).
    let settings = super::raid_settings::load(st, guild_id).await?;
    let default_tz = settings
        .realm_effective_time_zone
        .clone()
        .filter(|z| !z.is_empty())
        .unwrap_or_else(|| "UTC".into());
    let games = settings.realm_games.clone().unwrap_or_default();
    let tz_of = |r: &RaidRow| -> String {
        if let Some(z) = r.series_tz.as_deref().filter(|z| !z.is_empty()) {
            return z.to_string();
        }
        if let Some(g) = games.get(&r.game).filter(|g| !g.time_zone.is_empty()) {
            return g.time_zone.clone();
        }
        default_tz.clone()
    };
    let ids: Vec<i64> = raid_rows.iter().map(|r| r.id).collect();
    let tzs: Vec<String> = raid_rows.iter().map(tz_of).collect();
    let local: HashMap<i64, NaiveDateTime> = if ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "SELECT u.id, e.starts_at AT TIME ZONE u.tz
             FROM unnest($1::BIGINT[], $2::TEXT[]) AS u(id, tz) JOIN events e ON e.id = u.id",
        )
        .bind(&ids)
        .bind(&tzs)
        .fetch_all(db)
        .await?
        .into_iter()
        .collect()
    };
    let signups = sqlx::query_as::<_, Sign>(
        "SELECT s.event_id, s.user_id, COALESCE(gm.nickname, u.display_name) AS display_name,
                s.status, s.class, s.character_name, s.character_last_name, s.character_class
         FROM event_signups s
         JOIN users u ON u.id = s.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = $2 AND gm.user_id = s.user_id
         WHERE s.event_id = ANY($1)
         ORDER BY s.signed_at",
    )
    .bind(&ids)
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    let mut raids = Vec::new();
    for (row, tz) in raid_rows.into_iter().zip(tzs) {
        let local_start = local
            .get(&row.id)
            .copied()
            .unwrap_or_else(|| row.starts_at.naive_utc());
        let mine = signups
            .iter()
            .filter(|s| s.event_id == row.id)
            .cloned()
            .collect();
        raids.push(Raid {
            row,
            tz,
            local_start,
            signups: mine,
        });
    }
    Ok((raids, default_tz))
}

/// Every member's characters (from their profiles).
pub(crate) async fn member_chars(db: &PgPool, guild_id: i64) -> AppResult<Vec<MemberChar>> {
    Ok(sqlx::query_as::<_, MemberChar>(
        "SELECT wc.user_id, wc.name, wc.last_name, wc.class,
                COALESCE(gm.nickname, u.display_name) AS display_name
         FROM wow_characters wc
         JOIN guild_members gm ON gm.user_id = wc.user_id AND gm.guild_id = $1
         JOIN users u ON u.id = wc.user_id",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?)
}

/// Picks a raid for each row (the outhouse's Loot page): the raid that day
/// (or the day either side, for late nights); with several, the one the
/// character signed up to, else the one running at the time; rows that
/// still can't tell (disenchanted, unmatched) follow the rest of that day.
fn auto_assign(ctx: &Ctx, rows: &[GRow]) -> Vec<(Option<i64>, String)> {
    let mut out: Vec<(Option<i64>, String)> = Vec::with_capacity(rows.len());
    let mut pending: Vec<(usize, Vec<i64>)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let exact: Vec<&Raid> = ctx
            .raids
            .iter()
            .filter(|r| r.local_date() == row.date)
            .collect();
        let cands: Vec<&Raid> = if exact.is_empty() {
            ctx.raids.iter().filter(|r| r.near(row.date)).collect()
        } else {
            exact
        };
        if cands.is_empty() {
            out.push((None, "No raid that day".into()));
            continue;
        }
        if cands.len() == 1 {
            out.push((Some(cands[0].row.id), String::new()));
            continue;
        }
        let signed: Vec<&&Raid> = cands
            .iter()
            .filter(|r| match_in_raid(r, &row.key).is_some())
            .collect();
        if signed.len() == 1 {
            out.push((Some(signed[0].row.id), String::new()));
            continue;
        }
        if let Some(t) = row.time {
            let during: Vec<&&Raid> = cands.iter().filter(|r| r.during(row.date, t)).collect();
            if during.len() == 1 {
                out.push((Some(during[0].row.id), String::new()));
                continue;
            }
        }
        pending.push((i, cands.iter().map(|r| r.row.id).collect()));
        out.push((None, format!("{} raids that day: pick one", cands.len())));
    }
    for (i, cands) in pending {
        let date = rows[i].date;
        let mut that_day: Vec<i64> = rows
            .iter()
            .zip(out.iter())
            .filter(|(r, (e, _))| r.date == date && e.is_some_and(|e| cands.contains(&e)))
            .filter_map(|(_, (e, _))| *e)
            .collect();
        that_day.sort_unstable();
        that_day.dedup();
        if that_day.len() == 1 {
            out[i] = (Some(that_day[0]), String::new());
        }
    }
    out
}

// ---- Preview and import -----------------------------------------------------------

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct PreviewRow {
    pub line: usize,
    pub gargul_id: String,
    pub date: NaiveDate,
    /// "HH:MM" when the export had times.
    pub time: Option<String>,
    pub character: String,
    pub disenchanted: bool,
    pub item_id: i32,
    /// "" when the server doesn't know the item yet.
    pub item_name: String,
    pub quality: i32,
    pub offspec: bool,
    /// The raid it goes with (raid card: that raid; Loot page: the best guess).
    #[serde(with = "option_as_string")]
    pub event_id: Option<i64>,
    /// Why it has no raid ("No raid that day", "2 raids that day: pick one").
    pub reason: String,
    /// Raid card: on the raid's day (or the day either side).
    pub in_window: bool,
    /// Already imported (skipped).
    pub duplicate: bool,
    pub matched: Option<Matched>,
    /// The winner had soft-reserved it in that raid.
    pub reserved: bool,
}

#[derive(Serialize)]
pub struct PreviewRaid {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    /// The raid's day where it's played.
    pub local_date: NaiveDate,
    pub instance_name: String,
    pub game: String,
}

#[derive(Serialize)]
pub struct Preview {
    pub rows: Vec<PreviewRow>,
    pub raids: Vec<PreviewRaid>,
    pub errors: Vec<LineError>,
    pub duplicates: usize,
    /// The outhouse's rule for won soft reserves ("keep", "lock", "remove").
    pub sr_won: String,
}

fn preview_rows(ctx: &Ctx, rows: &[GRow], event: Option<i64>) -> Vec<PreviewRow> {
    let assigned: Vec<(Option<i64>, String)> = match event {
        Some(e) => rows.iter().map(|_| (Some(e), String::new())).collect(),
        None => auto_assign(ctx, rows),
    };
    rows.iter()
        .zip(assigned)
        .map(|(r, (event_id, reason))| {
            let raid = event_id.and_then(|e| ctx.raid(e));
            let matched = ctx.who(raid, r);
            let reserved = match (&matched, event_id) {
                (Some(m), Some(e)) => ctx.reserves.contains(&(e, m.user_id, r.item_id)),
                _ => false,
            };
            let (item_name, quality) = ctx
                .names
                .get(&r.item_id)
                .cloned()
                .unwrap_or((String::new(), 4));
            PreviewRow {
                line: r.line,
                gargul_id: r.gargul_id.clone(),
                date: r.date,
                time: r.time.map(|t| t.format("%H:%M").to_string()),
                character: r.character.clone(),
                disenchanted: r.disenchanted,
                item_id: r.item_id,
                item_name,
                quality,
                offspec: r.offspec,
                in_window: event.is_none() || raid.is_some_and(|x| x.near(r.date)),
                event_id,
                reason,
                duplicate: ctx.existing.contains(&r.gargul_id),
                matched,
                reserved,
            }
        })
        .collect()
}

/// Where a paste can go: one raid (`event`, its leaders) or the whole
/// outhouse (raid managers). The channels whose raids it can be matched to.
pub(crate) async fn import_scope(
    st: &AppState,
    user_id: i64,
    guild_id: Option<i64>,
    event: Option<i64>,
) -> AppResult<(i64, Vec<i64>)> {
    match event {
        Some(event_id) => {
            let (row, access) = load(st, event_id, user_id).await?;
            require_manager(st, &row, &access).await?;
            Ok((row.guild_id, vec![row.channel_id]))
        }
        None => {
            let guild_id = guild_id.ok_or(AppError::NotFound)?;
            outhouse_loot_manager(st, guild_id, user_id).await?;
            let ctx = member_ctx(&st.db, guild_id, user_id).await?;
            let channels = visible_text_channels(&st.db, &ctx, p::VIEW_CHANNEL).await?;
            Ok((guild_id, channels))
        }
    }
}

fn check_text(text: &str) -> AppResult<()> {
    if text.len() > MAX_TEXT {
        return Err(AppError::BadRequest(
            "That's too much to paste at once (1 MB). Export a shorter date range from Gargul."
                .into(),
        ));
    }
    Ok(())
}

async fn preview(
    st: &AppState,
    user_id: i64,
    guild_id: Option<i64>,
    event: Option<i64>,
    text: &str,
) -> AppResult<Preview> {
    check_text(text)?;
    let (guild_id, channels) = import_scope(st, user_id, guild_id, event).await?;
    let parsed = parse_gargul(text).map_err(AppError::BadRequest)?;
    let ctx = context(st, guild_id, &channels, event, &parsed.rows).await?;
    let rows = preview_rows(&ctx, &parsed.rows, event);
    // Unknown items: look them up now, so they have names by the time it's imported.
    let mut unknown: Vec<i32> = rows
        .iter()
        .filter(|r| r.item_name.is_empty())
        .map(|r| r.item_id)
        .collect();
    unknown.sort_unstable();
    unknown.dedup();
    fetch_names_later(st, unknown);
    let sr_won: Option<String> =
        sqlx::query_scalar("SELECT sr_won FROM raid_settings WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    Ok(Preview {
        duplicates: rows.iter().filter(|r| r.duplicate).count(),
        raids: ctx
            .raids
            .iter()
            .map(|r| PreviewRaid {
                id: r.row.id,
                channel_id: r.row.channel_id,
                title: r.row.title.clone(),
                starts_at: r.row.starts_at,
                local_date: r.local_date(),
                instance_name: raidplan::instance_name(&r.row.loot_instance),
                game: r.row.game.clone(),
            })
            .collect(),
        rows,
        errors: parsed.errors,
        sr_won: sr_won.unwrap_or_else(|| "lock".into()),
    })
}

/// `POST /api/guilds/{id}/loot/preview` `{text}` (raid managers): what an
/// import from the Loot page would do.
pub async fn guild_preview(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<PreviewRequest>,
) -> AppResult<Json<Preview>> {
    Ok(Json(
        preview(&st, user.id, Some(guild_id), None, &req.text).await?,
    ))
}

/// `POST /api/events/{id}/loot/preview` `{text}` (the raid's leaders): what
/// an import on this raid would do.
pub async fn raid_preview(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<PreviewRequest>,
) -> AppResult<Json<Preview>> {
    Ok(Json(
        preview(&st, user.id, None, Some(event_id), &req.text).await?,
    ))
}

#[derive(Deserialize)]
pub struct ImportPick {
    pub gargul_id: String,
    /// The raid it goes with (null: keep it without a raid). Ignored on a
    /// raid card's import (always that raid).
    #[serde(with = "option_as_string", default)]
    pub event_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub text: String,
    /// The rows to import (by Gargul ID).
    pub picks: Vec<ImportPick>,
}

#[derive(Serialize)]
pub struct ImportResult {
    #[serde(with = "option_as_string")]
    pub import_id: Option<i64>,
    pub imported: usize,
    pub duplicates: usize,
    pub without_raid: usize,
    pub raids: usize,
    /// Soft reserves won by this import.
    pub won_reserves: usize,
    /// …of which were taken off (the outhouse's "remove it" rule).
    pub removed_reserves: usize,
    pub undo_until: Option<DateTime<Utc>>,
}

/// A soft reserve taken off because it was won (kept for undo).
#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub(crate) struct RemovedReserve {
    event_id: i64,
    user_id: i64,
    item_id: i32,
    item_name: String,
    item_quality: i32,
    created_at: DateTime<Utc>,
}

/// The outhouse's rule for won soft reserves, applied to newly won ones
/// (event, user, item): "remove" takes them off (returned, for undo).
pub(crate) async fn apply_won_rule(
    st: &AppState,
    guild_id: i64,
    won: &[(i64, i64, i32)],
) -> AppResult<Vec<RemovedReserve>> {
    let rule: Option<String> =
        sqlx::query_scalar("SELECT sr_won FROM raid_settings WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?;
    let mut removed = Vec::new();
    if rule.as_deref() != Some("remove") {
        return Ok(removed);
    }
    for (event_id, user_id, item_id) in won {
        let gone = sqlx::query_as::<_, RemovedReserve>(
            "DELETE FROM loot_reserves WHERE event_id = $1 AND user_id = $2 AND item_id = $3
             RETURNING event_id, user_id, item_id, item_name, item_quality, created_at",
        )
        .bind(event_id)
        .bind(user_id)
        .bind(item_id)
        .fetch_optional(&st.db)
        .await?;
        removed.extend(gone);
    }
    Ok(removed)
}

async fn import(
    st: &AppState,
    user_id: i64,
    guild_id: Option<i64>,
    event: Option<i64>,
    req: ImportRequest,
) -> AppResult<ImportResult> {
    check_text(&req.text)?;
    let (guild_id, channels) = import_scope(st, user_id, guild_id, event).await?;
    let parsed = parse_gargul(&req.text).map_err(AppError::BadRequest)?;
    let ctx = context(st, guild_id, &channels, event, &parsed.rows).await?;
    let picks: HashMap<&str, Option<i64>> = req
        .picks
        .iter()
        .map(|p| (p.gargul_id.as_str(), event.or(p.event_id)))
        .collect();
    for raid in picks.values().flatten() {
        if ctx.raid(*raid).is_none() {
            return Err(AppError::BadRequest(
                "One of those raids isn't one you can import loot into.".into(),
            ));
        }
    }
    let chosen: Vec<(&GRow, Option<i64>)> = parsed
        .rows
        .iter()
        .filter_map(|r| picks.get(r.gargul_id.as_str()).map(|e| (r, *e)))
        .collect();
    if chosen.is_empty() {
        return Err(AppError::BadRequest(
            "Tick the rows to import first.".into(),
        ));
    }
    let import_id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO loot_imports (id, guild_id, event_id, imported_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(import_id)
    .bind(guild_id)
    .bind(event)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    let (mut imported, mut duplicates, mut without_raid) = (0usize, 0usize, 0usize);
    let mut won: Vec<(i64, i64, i32)> = Vec::new();
    let mut touched: Vec<i64> = Vec::new();
    let mut unnamed: Vec<i32> = Vec::new();
    for (row, event_id) in chosen {
        if ctx.existing.contains(&row.gargul_id) {
            duplicates += 1;
            continue;
        }
        let raid = event_id.and_then(|e| ctx.raid(e));
        let who = ctx.who(raid, row);
        let reserved = match (&who, raid) {
            (Some(m), Some(r)) => ctx.reserves.contains(&(r.row.id, m.user_id, row.item_id)),
            _ => false,
        };
        let (item_name, quality) = match ctx.names.get(&row.item_id) {
            Some(n) => n.clone(),
            None => {
                unnamed.push(row.item_id);
                (format!("Item {}", row.item_id), 4)
            }
        };
        let tz = raid
            .map(|r| r.tz.clone())
            .unwrap_or_else(|| ctx.default_tz.clone());
        let id = st.ids.next_id();
        let done = sqlx::query(
            "INSERT INTO loot_awards (id, guild_id, event_id, channel_id, raid_title, raid_starts_at,
                 loot_instance, user_id, character_name, character_key, character_class, item_id,
                 item_name, item_quality, reserved, note, awarded_by, awarded_at, awarded_on,
                 offspec, disenchanted, gargul_id, import_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, '', $16,
                 CASE WHEN $18::TIME IS NULL
                      THEN COALESCE($6, ($17::DATE + TIME '20:00') AT TIME ZONE $19)
                      ELSE ($17::DATE + $18::TIME) AT TIME ZONE $19 END,
                 $17, $20, $21, $22, $23)
             ON CONFLICT (guild_id, gargul_id) WHERE gargul_id IS NOT NULL DO NOTHING",
        )
        .bind(id)
        .bind(guild_id)
        .bind(raid.map(|r| r.row.id))
        .bind(raid.map(|r| r.row.channel_id))
        .bind(raid.map(|r| r.row.title.clone()).unwrap_or_default())
        .bind(raid.map(|r| r.row.starts_at))
        .bind(raid.map(|r| r.row.loot_instance.clone()).unwrap_or_default())
        .bind(who.as_ref().map(|m| m.user_id))
        .bind(&row.character)
        .bind(&row.key)
        .bind(who.as_ref().map(|m| m.class.clone()).unwrap_or_default())
        .bind(row.item_id)
        .bind(&item_name)
        .bind(quality)
        .bind(reserved)
        .bind(user_id)
        .bind(row.date)
        .bind(row.time)
        .bind(&tz)
        .bind(row.offspec)
        .bind(row.disenchanted)
        .bind(&row.gargul_id)
        .bind(import_id)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            duplicates += 1;
            continue;
        }
        imported += 1;
        match raid {
            Some(r) => {
                if !touched.contains(&r.row.id) {
                    touched.push(r.row.id);
                }
                if reserved {
                    if let Some(m) = &who {
                        won.push((r.row.id, m.user_id, row.item_id));
                    }
                }
            }
            None => without_raid += 1,
        }
    }
    if imported == 0 {
        // Nothing new: no import to undo.
        tx.rollback().await?;
        return Ok(ImportResult {
            import_id: None,
            imported: 0,
            duplicates,
            without_raid: 0,
            raids: 0,
            won_reserves: 0,
            removed_reserves: 0,
            undo_until: None,
        });
    }
    sqlx::query("UPDATE loot_imports SET row_count = $2 WHERE id = $1")
        .bind(import_id)
        .bind(imported as i32)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    won.sort_unstable();
    won.dedup();
    let removed = apply_won_rule(st, guild_id, &won).await?;
    if !removed.is_empty() {
        sqlx::query("UPDATE loot_imports SET removed_reserves = $2 WHERE id = $1")
            .bind(import_id)
            .bind(sqlx::types::Json(&removed))
            .execute(&st.db)
            .await?;
    }
    let mut details = format!(
        "Imported {imported} item{} from Gargul",
        if imported == 1 { "" } else { "s" }
    );
    let raid_names: Vec<String> = touched
        .iter()
        .filter_map(|id| ctx.raid(*id))
        .map(|r| format!("{} ({})", r.row.title, r.local_date().format("%-d %b")))
        .collect();
    if !raid_names.is_empty() {
        details.push_str(&format!(": {}", raid_names.join(", ")));
    }
    if without_raid > 0 {
        details.push_str(&format!(" · {without_raid} without a raid"));
    }
    if duplicates > 0 {
        details.push_str(&format!(" · {duplicates} already in"));
    }
    if !won.is_empty() {
        details.push_str(&format!(" · {} soft reserve(s) won", won.len()));
    }
    crate::audit::Entry::new(guild_id, user_id, "loot_import")
        .details(details)
        .save(st)
        .await;
    for event_id in &touched {
        broadcast(st, *event_id).await?;
    }
    unnamed.sort_unstable();
    unnamed.dedup();
    fetch_names_later(st, unnamed);
    Ok(ImportResult {
        import_id: Some(import_id),
        imported,
        duplicates,
        without_raid,
        raids: touched.len(),
        won_reserves: won.len(),
        removed_reserves: removed.len(),
        undo_until: Some(Utc::now() + chrono::Duration::hours(UNDO_HOURS)),
    })
}

/// `POST /api/guilds/{id}/loot/import` `{text, picks: [{gargul_id,
/// event_id}]}` (raid managers): imports the ticked rows, each with its raid
/// (or none).
pub async fn guild_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportResult>> {
    Ok(Json(import(&st, user.id, Some(guild_id), None, req).await?))
}

/// `POST /api/events/{id}/loot/import` `{text, picks: [{gargul_id}]}` (the
/// raid's leaders): imports the ticked rows into this raid.
pub async fn raid_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportResult>> {
    Ok(Json(import(&st, user.id, None, Some(event_id), req).await?))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ImportInfo {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "option_as_string")]
    pub event_id: Option<i64>,
    #[serde(with = "option_as_string")]
    pub imported_by: Option<i64>,
    pub imported_by_name: Option<String>,
    pub row_count: i32,
    /// Still in the loot history (some may have been deleted one by one).
    pub remaining: i64,
    pub created_at: DateTime<Utc>,
    pub undone_at: Option<DateTime<Utc>>,
    #[sqlx(skip)]
    pub can_undo: bool,
}

/// `GET /api/guilds/{id}/loot/imports` (raid managers): the latest imports.
pub async fn list_imports(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<ImportInfo>>> {
    outhouse_loot_manager(&st, guild_id, user.id).await?;
    let mut rows = sqlx::query_as::<_, ImportInfo>(
        "SELECT i.id, i.event_id, i.imported_by,
                COALESCE(gm.nickname, u.display_name) AS imported_by_name, i.row_count,
                (SELECT count(*) FROM loot_awards a WHERE a.import_id = i.id) AS remaining,
                i.created_at, i.undone_at
         FROM loot_imports i
         LEFT JOIN users u ON u.id = i.imported_by
         LEFT JOIN guild_members gm ON gm.guild_id = i.guild_id AND gm.user_id = i.imported_by
         WHERE i.guild_id = $1
         ORDER BY i.created_at DESC LIMIT 20",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let cutoff = Utc::now() - chrono::Duration::hours(UNDO_HOURS);
    for r in &mut rows {
        r.can_undo =
            r.imported_by == Some(user.id) && r.undone_at.is_none() && r.created_at > cutoff;
    }
    Ok(Json(rows))
}

/// `DELETE /api/loot/imports/{id}`: the importer takes a whole import back
/// out (within 24 hours), putting back any soft reserves it removed.
pub async fn undo_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(import_id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    #[allow(clippy::type_complexity)]
    let found: Option<(
        i64,
        Option<i64>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        sqlx::types::Json<Vec<RemovedReserve>>,
    )> = sqlx::query_as(
        "SELECT guild_id, imported_by, created_at, undone_at, removed_reserves
         FROM loot_imports WHERE id = $1",
    )
    .bind(import_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((guild_id, by, at, undone, removed)) = found else {
        return Err(AppError::NotFound);
    };
    member_ctx(&st.db, guild_id, user.id).await?;
    if by != Some(user.id) {
        return Err(AppError::Forbidden);
    }
    if undone.is_some() {
        return Err(AppError::BadRequest(
            "That import was already undone.".into(),
        ));
    }
    if at < Utc::now() - chrono::Duration::hours(UNDO_HOURS) {
        return Err(AppError::BadRequest(
            "Imports can be undone for 24 hours. Remove single items from the loot history instead."
                .into(),
        ));
    }
    let mut tx = st.db.begin().await?;
    let events: Vec<Option<i64>> =
        sqlx::query_scalar("DELETE FROM loot_awards WHERE import_id = $1 RETURNING event_id")
            .bind(import_id)
            .fetch_all(&mut *tx)
            .await?;
    for r in &removed.0 {
        sqlx::query(
            "INSERT INTO loot_reserves (event_id, user_id, item_id, item_name, item_quality, created_at)
             SELECT $1, $2, $3, $4, $5, $6 WHERE EXISTS (SELECT 1 FROM events WHERE id = $1)
             ON CONFLICT DO NOTHING",
        )
        .bind(r.event_id)
        .bind(r.user_id)
        .bind(r.item_id)
        .bind(&r.item_name)
        .bind(r.item_quality)
        .bind(r.created_at)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE loot_imports SET undone_at = now() WHERE id = $1")
        .bind(import_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::audit::Entry::new(guild_id, user.id, "loot_import_undo")
        .details(format!(
            "Undid a Gargul import ({} item{})",
            events.len(),
            if events.len() == 1 { "" } else { "s" }
        ))
        .save(&st)
        .await;
    let mut touched: Vec<i64> = events
        .iter()
        .flatten()
        .copied()
        .chain(removed.0.iter().map(|r| r.event_id))
        .collect();
    touched.sort_unstable();
    touched.dedup();
    for event_id in touched {
        // The raid may have been deleted since.
        let _ = broadcast(&st, event_id).await;
    }
    Ok(Json(serde_json::json!({ "removed": events.len() })))
}

// ---- Loot history: filters, sorting, summary, CSV ------------------------------------

#[derive(Deserialize, Default)]
pub struct HistoryQuery {
    user: Option<String>,
    character: Option<String>,
    q: Option<String>,
    from: Option<String>,
    to: Option<String>,
    /// A raid's ID, or "none" (kept without a raid).
    event: Option<String>,
    instance: Option<String>,
    /// "ms" or "os".
    spec: Option<String>,
    /// "1": include disenchanted items.
    de: Option<String>,
    /// "date" (default), "item", "character" or "raid".
    sort: Option<String>,
    /// "asc" or "desc" (default).
    dir: Option<String>,
    before: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
}

struct Filters {
    user: Option<i64>,
    character: Option<String>,
    text: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    event: Option<Option<i64>>,
    instance: Option<String>,
    spec: Option<&'static str>,
    de: bool,
    before: Option<i64>,
}

fn like(t: &str) -> String {
    format!(
        "%{}%",
        t.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

fn filters(q: &HistoryQuery) -> AppResult<Filters> {
    let id = |v: &Option<String>| -> AppResult<Option<i64>> {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => s
                .parse()
                .map(Some)
                .map_err(|_| AppError::BadRequest("Bad ID.".into())),
        }
    };
    let day = |v: &Option<String>| -> AppResult<Option<NaiveDate>> {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| AppError::BadRequest("Dates are YYYY-MM-DD.".into())),
        }
    };
    let text = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(|t| t.trim().chars().take(100).collect::<String>())
            .filter(|t| !t.is_empty())
    };
    Ok(Filters {
        user: id(&q.user)?,
        character: text(&q.character)
            .map(|c| char_key(&c))
            .filter(|c| !c.is_empty()),
        text: text(&q.q).map(|t| like(&t)),
        from: day(&q.from)?,
        to: day(&q.to)?,
        event: match q.event.as_deref().map(str::trim) {
            None | Some("") => None,
            Some("none") => Some(None),
            Some(_) => Some(id(&q.event)?),
        },
        instance: text(&q.instance).map(|t| t.chars().take(60).collect()),
        spec: match q.spec.as_deref() {
            Some("ms") => Some("ms"),
            Some("os") => Some("os"),
            _ => None,
        },
        de: matches!(q.de.as_deref(), Some("1" | "true" | "yes")),
        before: id(&q.before)?,
    })
}

/// Raids in channels they can see; loot without a raid if they can see a
/// raid channel at all (or run the outhouse's raids).
pub(crate) async fn history_scope(st: &AppState, ctx: &MemberCtx) -> AppResult<(Vec<i64>, bool)> {
    let mut channels = visible_text_channels(&st.db, ctx, p::VIEW_CHANNEL).await?;
    drop_age_gated(&st.db, ctx.user_id, &mut channels).await?;
    let manager = ctx.require(p::MANAGE_EVENTS).is_ok()
        || super::raid_settings::has_manager_role(&st.db, ctx).await?;
    let raidless = manager
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM events WHERE guild_id = $1 AND channel_id = ANY($2))",
        )
        .bind(ctx.guild_id)
        .bind(&channels)
        .fetch_one(&st.db)
        .await?;
    Ok((channels, raidless))
}

fn push_where(
    qb: &mut QueryBuilder<'_, Postgres>,
    guild_id: i64,
    channels: &[i64],
    raidless: bool,
    f: &Filters,
) {
    qb.push(" WHERE a.guild_id = ");
    qb.push_bind(guild_id);
    qb.push(" AND (a.channel_id = ANY(");
    qb.push_bind(channels.to_vec());
    qb.push(")");
    if raidless {
        qb.push(" OR a.channel_id IS NULL");
    }
    qb.push(")");
    if let Some(u) = f.user {
        qb.push(" AND a.user_id = ");
        qb.push_bind(u);
    }
    if let Some(c) = &f.character {
        qb.push(" AND a.character_key LIKE ");
        qb.push_bind(like(c));
    }
    if let Some(t) = &f.text {
        qb.push(" AND (a.item_name ILIKE ");
        qb.push_bind(t.clone());
        qb.push(" OR a.raid_title ILIKE ");
        qb.push_bind(t.clone());
        qb.push(" OR a.character_name ILIKE ");
        qb.push_bind(t.clone());
        qb.push(" OR COALESCE(gm.nickname, u.display_name) ILIKE ");
        qb.push_bind(t.clone());
        qb.push(")");
    }
    if let Some(d) = f.from {
        qb.push(" AND a.awarded_on >= ");
        qb.push_bind(d);
    }
    if let Some(d) = f.to {
        qb.push(" AND a.awarded_on <= ");
        qb.push_bind(d);
    }
    match f.event {
        Some(Some(e)) => {
            qb.push(" AND a.event_id = ");
            qb.push_bind(e);
        }
        Some(None) => {
            qb.push(" AND a.event_id IS NULL");
        }
        None => {}
    }
    if let Some(i) = &f.instance {
        qb.push(" AND lower(a.loot_instance) = lower(");
        qb.push_bind(i.clone());
        qb.push(")");
    }
    match f.spec {
        Some("ms") => {
            qb.push(" AND NOT a.offspec AND NOT a.disenchanted");
        }
        Some("os") => {
            qb.push(" AND a.offspec");
        }
        _ => {}
    }
    if !f.de {
        qb.push(" AND NOT a.disenchanted");
    }
    if let Some(b) = f.before {
        qb.push(" AND a.id < ");
        qb.push_bind(b);
    }
}

fn push_order(qb: &mut QueryBuilder<'_, Postgres>, q: &HistoryQuery) {
    let dir = if q.dir.as_deref() == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };
    let by = match q.sort.as_deref() {
        Some("item") => format!("lower(a.item_name) {dir}, a.awarded_on DESC"),
        Some("character") => format!("lower(a.character_name) {dir}, a.awarded_on DESC"),
        Some("raid") => format!("a.raid_starts_at {dir} NULLS LAST, a.awarded_on {dir}"),
        _ => format!("a.awarded_on {dir}, a.awarded_at {dir}"),
    };
    qb.push(format!(" ORDER BY {by}, a.id {dir}"));
}

#[derive(Serialize)]
pub struct LootLog {
    pub entries: Vec<LootAward>,
    pub has_more: bool,
    pub total: i64,
}

/// `GET /api/guilds/{id}/loot?user=&character=&q=&from=&to=&event=&instance=&spec=&de=&sort=&dir=&offset=&limit=`:
/// the outhouse's loot history (raids in channels you can see).
pub async fn guild_log(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<LootLog>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let (channels, raidless) = history_scope(&st, &ctx).await?;
    let f = filters(&q)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).clamp(0, 100_000);
    let mut count = QueryBuilder::<Postgres>::new(
        "SELECT count(*) FROM loot_awards a
         LEFT JOIN users u ON u.id = a.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = a.guild_id AND gm.user_id = a.user_id",
    );
    push_where(&mut count, guild_id, &channels, raidless, &f);
    let total: i64 = count.build_query_scalar().fetch_one(&st.db).await?;
    let mut qb = QueryBuilder::<Postgres>::new(AWARD_SELECT);
    push_where(&mut qb, guild_id, &channels, raidless, &f);
    push_order(&mut qb, &q);
    qb.push(" LIMIT ");
    qb.push_bind(limit + 1);
    qb.push(" OFFSET ");
    qb.push_bind(offset);
    let mut rows: Vec<AwardRow> = qb.build_query_as().fetch_all(&st.db).await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let mut entries: Vec<LootAward> = rows.into_iter().map(LootAward::from).collect();
    fill_branches(&st, &mut entries).await;
    Ok(Json(LootLog {
        entries,
        has_more,
        total,
    }))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct CharacterSummary {
    pub character_key: String,
    pub character_name: String,
    #[serde(with = "option_as_string")]
    pub user_id: Option<i64>,
    pub display_name: Option<String>,
    pub character_class: String,
    pub ms: i64,
    pub os: i64,
    pub reserved: i64,
    pub total: i64,
    pub last_on: NaiveDate,
}

/// `GET /api/guilds/{id}/loot/summary?…`: per character, items won main
/// spec and off spec and the last one's date (same filters as the history;
/// disenchanted items aren't anyone's).
pub async fn guild_summary(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<Vec<CharacterSummary>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let (channels, raidless) = history_scope(&st, &ctx).await?;
    let mut f = filters(&q)?;
    f.de = false;
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT a.character_key, max(a.character_name) AS character_name, a.user_id,
                max(COALESCE(gm.nickname, u.display_name)) AS display_name,
                max(a.character_class) AS character_class,
                count(*) FILTER (WHERE NOT a.offspec) AS ms,
                count(*) FILTER (WHERE a.offspec) AS os,
                count(*) FILTER (WHERE a.reserved) AS reserved,
                count(*) AS total, max(a.awarded_on) AS last_on
         FROM loot_awards a
         LEFT JOIN users u ON u.id = a.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = a.guild_id AND gm.user_id = a.user_id",
    );
    push_where(&mut qb, guild_id, &channels, raidless, &f);
    qb.push(" GROUP BY a.character_key, a.user_id ORDER BY total DESC, max(a.awarded_on) DESC LIMIT 500");
    let rows: Vec<CharacterSummary> = qb.build_query_as().fetch_all(&st.db).await?;
    Ok(Json(rows))
}

pub const EXPORT_HEADER: &str = "Date,Character,Member,Class,Item ID,Item,Offspec,Disenchanted,Soft reserved,Raid,Instance,Gargul ID\n";

/// `GET /api/guilds/{id}/loot/export?…` → `{csv, rows}`: the history as CSV
/// (same filters, oldest first).
pub async fn guild_export(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(mut q): Query<HistoryQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let (channels, raidless) = history_scope(&st, &ctx).await?;
    let f = filters(&q)?;
    if q.sort.is_none() {
        q.dir = Some("asc".into());
    }
    let mut qb = QueryBuilder::<Postgres>::new(AWARD_SELECT);
    push_where(&mut qb, guild_id, &channels, raidless, &f);
    push_order(&mut qb, &q);
    qb.push(" LIMIT 20000");
    let rows: Vec<AwardRow> = qb.build_query_as().fetch_all(&st.db).await?;
    let mut csv = String::from(EXPORT_HEADER);
    let n = rows.len();
    for a in rows.into_iter().map(LootAward::from) {
        let yes = |b: bool| if b { "1" } else { "0" };
        let line = [
            a.awarded_on.to_string(),
            csv_field(&a.character_name),
            csv_field(&a.display_name),
            csv_field(&a.character_class),
            a.item_id.to_string(),
            csv_field(&a.item_name),
            yes(a.offspec).into(),
            yes(a.disenchanted).into(),
            yes(a.reserved).into(),
            csv_field(&a.raid_title),
            csv_field(&a.instance_name),
            csv_field(a.gargul_id.as_deref().unwrap_or("")),
        ]
        .join(",");
        csv.push_str(&line);
        csv.push('\n');
    }
    Ok(Json(serde_json::json!({ "csv": csv, "rows": n })))
}

// ---- A member's loot, by character (full profile) -------------------------------------

/// Raid tiers ("phases"): (era, phase) of a listed raid.
pub(crate) fn phase_of(instance: &str) -> Option<(&'static str, u8)> {
    Some(match raidplan::instance_key(instance).as_str() {
        "mc" | "ony" => ("classic", 1),
        "bwl" => ("classic", 3),
        "zg" => ("classic", 4),
        "aq20" | "aq40" => ("classic", 5),
        "naxx" => ("classic", 6),
        "kara" | "gruul" | "mag" => ("tbc", 1),
        "ssc" | "tk" => ("tbc", 2),
        "hyjal" | "bt" => ("tbc", 3),
        "za" => ("tbc", 4),
        "swp" => ("tbc", 5),
        _ => return None,
    })
}

#[derive(Serialize)]
pub struct CharacterLoot {
    pub character_name: String,
    pub character_key: String,
    pub character_class: String,
    pub total: usize,
    pub offspec: usize,
    /// Items from the raids of the latest phase they've had loot from (or
    /// the last two months, when the raids aren't listed ones).
    pub this_phase: usize,
    /// "Phase 3" or "last 2 months".
    pub phase_label: String,
    pub items: Vec<LootAward>,
}

#[derive(Serialize)]
pub struct OuthouseLoot {
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub guild_name: String,
    pub characters: Vec<CharacterLoot>,
}

fn by_character(awards: Vec<LootAward>, today: NaiveDate) -> Vec<CharacterLoot> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<LootAward>> = HashMap::new();
    for a in awards {
        let key = if a.character_name.is_empty() {
            String::new()
        } else {
            char_key(&a.character_name)
        };
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(a);
    }
    order
        .into_iter()
        .map(|key| {
            let items = groups.remove(&key).unwrap_or_default();
            // The highest phase they've had loot from (in the era of their latest).
            let era = items
                .iter()
                .find_map(|a| phase_of(&a.loot_instance))
                .map(|p| p.0);
            let latest = era.and_then(|era| {
                items
                    .iter()
                    .filter_map(|a| phase_of(&a.loot_instance))
                    .filter(|p| p.0 == era)
                    .max_by_key(|p| p.1)
            });
            let (this_phase, phase_label) = match latest {
                Some(ph) => (
                    items
                        .iter()
                        .filter(|a| phase_of(&a.loot_instance) == Some(ph))
                        .count(),
                    format!("Phase {}", ph.1),
                ),
                None => (
                    items
                        .iter()
                        .filter(|a| (today - a.awarded_on).num_days() <= 61)
                        .count(),
                    "last 2 months".to_string(),
                ),
            };
            CharacterLoot {
                character_name: items
                    .iter()
                    .map(|a| a.character_name.clone())
                    .find(|n| !n.is_empty())
                    .unwrap_or_default(),
                character_class: items
                    .iter()
                    .map(|a| a.character_class.clone())
                    .find(|c| !c.is_empty())
                    .unwrap_or_default(),
                character_key: key,
                total: items.len(),
                offspec: items.iter().filter(|a| a.offspec).count(),
                this_phase,
                phase_label,
                items,
            }
        })
        .collect()
}

/// `GET /api/users/{id}/loot`: what they've won, by character, in the
/// outhouses you're both in (raids in channels you can see).
pub async fn user_loot(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(target): Path<i64>,
) -> AppResult<Json<Vec<OuthouseLoot>>> {
    let shared: Vec<(i64, String)> = sqlx::query_as(
        "SELECT g.id, g.name FROM guild_members me
         JOIN guild_members them ON them.guild_id = me.guild_id AND them.user_id = $2
         JOIN guilds g ON g.id = me.guild_id
         WHERE me.user_id = $1
         ORDER BY g.name",
    )
    .bind(user.id)
    .bind(target)
    .fetch_all(&st.db)
    .await?;
    let today = Utc::now().date_naive();
    let mut out = Vec::new();
    for (guild_id, guild_name) in shared {
        let Ok(ctx) = member_ctx(&st.db, guild_id, user.id).await else {
            continue;
        };
        let (channels, raidless) = history_scope(&st, &ctx).await?;
        let sql = format!(
            "{AWARD_SELECT} WHERE a.guild_id = $1 AND a.user_id = $2 AND NOT a.disenchanted
               AND (a.channel_id = ANY($3) OR ($4 AND a.channel_id IS NULL))
             ORDER BY a.awarded_on DESC, a.awarded_at DESC, a.id DESC LIMIT 300"
        );
        let rows = sqlx::query_as::<_, AwardRow>(&sql)
            .bind(guild_id)
            .bind(target)
            .bind(&channels)
            .bind(raidless)
            .fetch_all(&st.db)
            .await?;
        if rows.is_empty() {
            continue;
        }
        let mut awards: Vec<LootAward> = rows.into_iter().map(LootAward::from).collect();
        fill_branches(&st, &mut awards).await;
        fill_bosses(&st.db, guild_id, &mut awards).await;
        out.push(OuthouseLoot {
            guild_id,
            guild_name,
            characters: by_character(awards, today),
        });
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "dateTime,character,itemID,offspec,id
2026-11-08,Shamièè,32240,1,12811834116478231330
2026-11-08,_disenchanted,32254,0,77673436522395093700
2026-11-09,Danité,31091,0,33618872553272877036
";

    #[test]
    fn reads_the_owners_export() {
        let p = parse_gargul(SAMPLE).unwrap();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.rows.len(), 3);
        let r = &p.rows[0];
        assert_eq!(r.character, "Shamièè");
        assert_eq!(r.key, "shamiee");
        assert!(r.offspec && !r.disenchanted);
        assert_eq!(r.item_id, 32240);
        assert_eq!(r.date, NaiveDate::from_ymd_opt(2026, 11, 8).unwrap());
        assert_eq!(r.time, None);
        assert_eq!(r.gargul_id, "12811834116478231330");
        assert!(p.rows[1].disenchanted && !p.rows[1].offspec && p.rows[1].key.is_empty());
        assert_eq!(p.rows[2].key, "danite");
    }

    #[test]
    fn lenient_about_the_format() {
        // BOM, CRLF, quotes, extra and reordered columns, a full date-time,
        // decomposed accents (NFD) and an item link.
        let text = "\u{feff}\"id\",\"itemLink\",\"character\",\"dateTime\",\"OS\",\"note\"\r\n\
                    \"a1\",\"|cffa335ee|Hitem:32240::::::::70:::::|h[Guise of the Tidal Lurker]|h|r\",\"Shamie\u{300}e\u{300}\",\"2026-11-08 23:41:07\",\"0\",\"says \"\"hi\"\", ok\"\r\n\
                    \r\n\
                    \"a2\",32254,Druixxe-Gehennas,08/11/2026,1,\r\n";
        let p = parse_gargul(text).unwrap();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].item_id, 32240);
        assert_eq!(p.rows[0].character, "Shamièè");
        assert_eq!(p.rows[0].key, "shamiee");
        assert_eq!(
            p.rows[0].time,
            Some(NaiveTime::from_hms_opt(23, 41, 7).unwrap())
        );
        assert_eq!(p.rows[1].key, "druixxe");
        assert!(p.rows[1].offspec);
        assert_eq!(
            p.rows[1].date,
            NaiveDate::from_ymd_opt(2026, 11, 8).unwrap()
        );
    }

    #[test]
    fn bad_rows_are_listed() {
        let text = "dateTime,character,itemID,offspec,id\n2026-11-08,,1,0,x\nyesterday,Bob,5,0,y\n2026-11-08,Bob,abc,0,z\n2026-11-08,Bob,5,0,w\n2026-11-08,Bob,6,0,w\n";
        let p = parse_gargul(text).unwrap();
        assert_eq!(p.rows.len(), 1);
        assert_eq!(
            p.errors.iter().map(|e| e.line).collect::<Vec<_>>(),
            vec![2, 3, 4, 6]
        );
        assert!(parse_gargul("hello world").is_err());
        assert!(parse_gargul("").is_err());
    }

    #[test]
    fn no_id_column_still_dedupes() {
        let text = "date,player,item\n2026-11-08,Bob,5\n2026-11-08,Bob,5\n";
        let a = parse_gargul(text).unwrap();
        let b = parse_gargul(text).unwrap();
        assert_eq!(a.rows.len(), 2);
        assert_ne!(a.rows[0].gargul_id, a.rows[1].gargul_id);
        assert_eq!(a.rows[1].gargul_id, b.rows[1].gargul_id);
    }

    #[test]
    fn names_match_without_case_or_accents() {
        assert_eq!(char_key("  DANITÉ "), "danite");
        assert!(name_is("danite", "Danité", ""));
        assert!(name_is("aldric stormborn", "Aldric", "Stormborn"));
        assert!(name_is("aldric", "Aldric", "Stormborn"));
        assert!(!name_is("aldric", "Aldrik", ""));
        assert!(same_character("", "Bob", ""));
        assert!(same_character("bob", "", ""));
        assert!(!same_character("bob", "Alice", ""));
    }
}
