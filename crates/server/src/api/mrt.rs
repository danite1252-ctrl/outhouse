//! Raid attendance from Method Raid Tools (MRT).
//!
//! MRT's attendance export is a tab-separated grid: the first line has an
//! empty corner cell, then one column per boss pull, headed
//! "15/11/2026 20:44:00 - High Warlord Naj'entus4" (day/month/year in the
//! exporter's local time, the boss, and MRT's difficulty ID glued on the
//! end); each following line is a character and an "x" under every boss
//! they were there for. Raid leaders paste it on a raid card or in the
//! outhouse's Loot & attendance window; each column goes to a raid (that
//! raid, or by date, time and who signed up), each name to a member (by
//! character, without case, accents or "-Realm").
//!
//! The difficulty number is only split off the name when the rest is a boss
//! on the encounter lists (or another column in the same paste confirmed
//! that number), so a boss whose real name ends in a digit stays whole.
//!
//! Also here: a raid's attendance (its card: imported bosses and the marks
//! raid leaders set by hand, see `attendance.rs`), the per-character
//! attendance numbers (the Loot window's By character tab and full
//! profiles), and the no-show hint raid leaders see on sign-ups.
//!
//! Counting: a raid *has attendance* when it has imported bosses or hand
//! marks. Someone *attended* if they were there for at least one boss, or
//! were marked present, late or benched. A *no-show* signed up (yes, late or
//! tentative) to a raid with attendance and wasn't there for any boss (or
//! was marked absent); "can't make it", the bench and anyone marked excused
//! never count. A hand mark overrides the import for that member on that
//! raid (the `raid_attendance_outcomes` view, migration 0047). Attendance %
//! is raids attended out of raids they attended or no-showed.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use outhouse_common::id::{as_string, option_as_string};
use outhouse_common::raidplan::{self, Encounter, INSTANCE_ZONES};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::events::{broadcast, can_manage, load};
use super::loot_import::{
    char_key, history_scope, import_scope, match_in_raid, match_member, member_chars, phase_of,
    raids_between, LineError, Matched, MemberChar, Raid,
};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{drop_age_gated, member_ctx, visible_text_channels};
use crate::state::AppState;

/// Biggest paste (an export of a long night is a few kilobytes).
const MAX_TEXT: usize = 500_000;
const MAX_COLUMNS: usize = 200;
const MAX_PLAYERS: usize = 400;
/// How long the importer can undo an import.
const UNDO_HOURS: i64 = 24;
/// The same boss within this many seconds in the same raid is the same pull
/// (two leaders' exports are a few seconds apart).
const SAME_PULL_SECS: i64 = 120;
/// The no-show hint on sign-ups: this many no-shows …
pub const HINT_NO_SHOWS: i64 = 2;
/// … in this many days.
pub const HINT_DAYS: i64 = 28;

// ---- Reading MRT's export ----------------------------------------------------------

/// One column: a boss pull.
#[derive(Debug, Clone)]
pub(crate) struct Column {
    /// Line it was headed on.
    pub line: usize,
    /// When, in the exporter's local time.
    pub local: NaiveDateTime,
    /// The boss as MRT wrote it (difficulty number taken off).
    pub boss: String,
    /// What re-imports are matched on: the encounter, or the name.
    pub boss_key: String,
    /// ("bt", the encounter) when the boss is on the encounter lists.
    pub encounter: Option<(&'static str, &'static Encounter)>,
    /// MRT's difficulty ID (3 = 10-player, 4 = 25-player, 9 = 40-player, …).
    pub difficulty: Option<i32>,
}

#[derive(Debug, Clone)]
pub(crate) struct Player {
    pub line: usize,
    pub name: String,
    pub key: String,
    /// Per column: Some(true) there, Some(false) listed but not there,
    /// None not listed under that column at all (another night's grid).
    pub present: Vec<Option<bool>>,
}

#[derive(Debug, Default)]
pub(crate) struct Grid {
    pub columns: Vec<Column>,
    pub players: Vec<Player>,
    pub warnings: Vec<LineError>,
    /// Lines before the first header (a raid name, say), left out.
    pub ignored: Vec<String>,
}

/// Letters and digits only, lower case, accents off: how boss names are compared.
fn slug(raw: &str) -> String {
    raw.nfd()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The boss on the encounter lists with this name (or one of its NPCs' names).
pub(crate) fn find_encounter(name: &str) -> Option<(&'static str, &'static Encounter)> {
    let want = slug(name);
    if want.is_empty() {
        return None;
    }
    INSTANCE_ZONES.iter().find_map(|(key, _, list)| {
        list.iter()
            .find(|e| slug(e.name) == want || e.aliases.iter().any(|a| slug(a) == want))
            .map(|e| (*key, e))
    })
}

fn encounter_key(inst: &str, e: &Encounter) -> String {
    format!("{inst}:{}", slug(e.name))
}

/// "High Warlord Naj'entus4" → ("High Warlord Naj'entus", Some(4), the
/// encounter). Digits are only split off when they're glued to the name and
/// what's left is a boss on the lists; otherwise the name stays whole.
pub(crate) fn split_boss(
    raw: &str,
) -> (
    String,
    Option<i32>,
    Option<(&'static str, &'static Encounter)>,
) {
    let raw = raw.trim();
    if let Some(e) = find_encounter(raw) {
        return (raw.to_string(), None, Some(e));
    }
    if let Some((rest, d)) = glued_digits(raw, 3) {
        if let Some(e) = find_encounter(rest) {
            return (rest.trim().to_string(), Some(d), Some(e));
        }
    }
    (raw.to_string(), None, None)
}

/// A number of up to `max` digits stuck straight onto a name ("Boss4", not
/// "Boss 4" or "Boss 3000").
fn glued_digits(raw: &str, max: usize) -> Option<(&str, i32)> {
    let n = raw.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 || n > max || n == raw.len() {
        return None;
    }
    let (rest, digits) = raw.split_at(raw.len() - n);
    let before = rest.chars().last()?;
    if before.is_whitespace() || before.is_ascii_digit() || before == '-' {
        return None;
    }
    Some((rest, digits.parse().ok()?))
}

/// A difficulty ID in words.
pub(crate) fn difficulty_label(d: i32) -> &'static str {
    match d {
        1 => "Normal",
        2 => "Heroic",
        3 => "10-player",
        4 => "25-player",
        5 => "10-player heroic",
        6 => "25-player heroic",
        9 => "40-player",
        14 => "Normal",
        15 => "Heroic",
        16 => "Mythic",
        148 => "20-player",
        _ => "",
    }
}

/// "15/11/2026 20:44:00 - High Warlord Naj'entus4" → its time and the boss
/// part. MRT writes the exporter's day/month/year.
fn parse_header(cell: &str) -> Option<(NaiveDateTime, String)> {
    let cell = cell.trim();
    const FORMATS: [&str; 10] = [
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%d/%m/%y %H:%M:%S",
        "%d/%m/%y %H:%M",
        "%d.%m.%Y %H:%M:%S",
        "%d.%m.%Y %H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
    ];
    let mut from = 0;
    while let Some(at) = cell[from..].find(" - ") {
        let split = from + at;
        let when = cell[..split].trim();
        let boss = cell[split + 3..].trim();
        if !boss.is_empty() {
            for f in FORMATS {
                if let Ok(t) = NaiveDateTime::parse_from_str(when, f) {
                    let y = chrono::Datelike::year(&t);
                    if (2004..=2100).contains(&y) {
                        return Some((t, boss.to_string()));
                    }
                }
            }
        }
        from = split + 3;
    }
    None
}

/// Cells of a line: tabs if it has any, else runs of two or more spaces
/// (a paste whose tabs became spaces). A leading gap is an empty first cell.
fn split_cells(line: &str) -> (Vec<String>, bool) {
    if line.contains('\t') {
        return (
            line.split('\t').map(|c| c.trim().to_string()).collect(),
            true,
        );
    }
    let mut out = Vec::new();
    if line.starts_with(char::is_whitespace) {
        out.push(String::new());
    }
    let mut cur = String::new();
    let mut spaces = 0;
    for c in line.trim().chars() {
        if c == ' ' || c == '\u{a0}' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        } else if spaces == 1 {
            cur.push(' ');
        }
        spaces = 0;
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    (out, false)
}

/// A mark under a boss: anything but empty, "0", "-" or "no" means there.
fn is_mark(cell: &str) -> bool {
    let c = cell.trim().to_lowercase();
    !(c.is_empty() || c == "0" || c == "-" || c == "no" || c == "false")
}

/// "Name x x" with single spaces: the name, then the short marks after it.
fn split_single_spaced(cell: &str) -> (String, Vec<String>) {
    let words: Vec<&str> = cell.split_whitespace().collect();
    let marks = words
        .iter()
        .rev()
        .take_while(|w| {
            (w.chars().count() <= 2 && !w.chars().any(char::is_alphabetic))
                || **w == "x"
                || **w == "X"
        })
        .count();
    if marks == 0 || marks == words.len() {
        return (cell.to_string(), Vec::new());
    }
    let name = words[..words.len() - marks].join(" ");
    let rest = words[words.len() - marks..]
        .iter()
        .map(|w| w.to_string())
        .collect();
    (name, rest)
}

/// Reads MRT's attendance export. Several grids (nights) in one paste are
/// fine: each header line starts a new set of columns.
pub(crate) fn parse_mrt(text: &str) -> Result<Grid, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Err("Paste MRT's attendance export first.".into());
    }
    let mut grid = Grid::default();
    // The current grid: its columns (global index) and the cell each is in.
    let mut block: Vec<(usize, usize)> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut spaced_warned = false;
    for (i, raw) in text.split('\n').enumerate() {
        let line = i + 1;
        let raw = raw.trim_end_matches('\r');
        if raw.trim().is_empty() {
            continue;
        }
        let (cells, tabbed) = split_cells(raw);
        let heads: Vec<(usize, NaiveDateTime, String)> = cells
            .iter()
            .enumerate()
            .filter_map(|(at, c)| parse_header(c).map(|(t, b)| (at, t, b)))
            .collect();
        if !heads.is_empty() {
            let first_at = heads[0].0;
            block.clear();
            for (at, local, boss_raw) in heads {
                if grid.columns.len() >= MAX_COLUMNS {
                    return Err(format!("That's more than {MAX_COLUMNS} bosses; paste a night or two at a time, please."));
                }
                let (boss, difficulty, encounter) = split_boss(&boss_raw);
                grid.columns.push(Column {
                    line,
                    local,
                    boss_key: encounter
                        .map(|(k, e)| encounter_key(k, e))
                        .unwrap_or_else(|| format!("name:{}", slug(&boss))),
                    boss,
                    encounter,
                    difficulty,
                });
                for p in &mut grid.players {
                    p.present.push(None);
                }
                // Rows have the name first, then one cell per column.
                block.push((grid.columns.len() - 1, at - first_at + 1));
            }
            continue;
        }
        if block.is_empty() {
            grid.ignored.push(raw.trim().chars().take(100).collect());
            continue;
        }
        let mut cells = cells;
        let mut name = cells.first().cloned().unwrap_or_default();
        if !tabbed && cells.len() == 1 {
            let (n, marks) = split_single_spaced(&name);
            name = n;
            cells = std::iter::once(name.clone()).chain(marks).collect();
        }
        let name: String = name.trim().nfc().collect();
        let key = char_key(&name);
        if key.is_empty() {
            grid.warnings.push(LineError {
                line,
                message: "No character name, so it's left out.".into(),
            });
            continue;
        }
        let marks = cells.len().saturating_sub(1);
        if !tabbed && block.len() > 1 && marks > 0 && marks < block.len() && !spaced_warned {
            spaced_warned = true;
            grid.warnings.push(LineError {
                line,
                message: "The paste has spaces instead of tabs, so empty cells can't be told apart: marks were read left to right. Check the bosses below (copying straight from MRT keeps the tabs).".into(),
            });
        }
        let idx = match by_key.get(&key) {
            Some(&i) => i,
            None => {
                if grid.players.len() >= MAX_PLAYERS {
                    return Err(format!("That's more than {MAX_PLAYERS} characters."));
                }
                grid.players.push(Player {
                    line,
                    name: name.clone(),
                    key: key.clone(),
                    present: vec![None; grid.columns.len()],
                });
                by_key.insert(key, grid.players.len() - 1);
                grid.players.len() - 1
            }
        };
        let p = &mut grid.players[idx];
        for &(col, cell) in &block {
            let here = cells.get(cell).is_some_and(|c| is_mark(c));
            p.present[col] = Some(p.present[col].unwrap_or(false) || here);
        }
    }
    if grid.columns.is_empty() {
        return Err("That doesn't look like MRT's attendance export: it needs a header line with columns like \"15/11/2026 20:44:00 - Boss name\".".into());
    }
    if grid.players.is_empty() {
        return Err("There's nobody under the bosses: paste the whole grid, names and all.".into());
    }
    // Digits on bosses that aren't on the lists: the same number another
    // column was confirmed to have is the difficulty too.
    let confirmed: HashSet<i32> = grid.columns.iter().filter_map(|c| c.difficulty).collect();
    for c in &mut grid.columns {
        if c.difficulty.is_none() && c.encounter.is_none() {
            if let Some((rest, d)) = glued_digits(&c.boss, 2) {
                if confirmed.contains(&d) {
                    c.boss = rest.trim().to_string();
                    c.boss_key = format!("name:{}", slug(&c.boss));
                    c.difficulty = Some(d);
                }
            }
        }
    }
    Ok(grid)
}

// ---- Columns to raids ------------------------------------------------------------

/// Picks a raid for each column: the raid running then, else the one that
/// day (or the day either side, for late nights); with several, the one
/// most of the people there signed up to; columns that still can't tell
/// follow the rest of that night.
fn auto_assign(raids: &[Raid], grid: &Grid) -> Vec<(Option<i64>, String)> {
    let mut out: Vec<(Option<i64>, String)> = Vec::new();
    let mut pending: Vec<(usize, Vec<i64>)> = Vec::new();
    for (ci, col) in grid.columns.iter().enumerate() {
        let (d, t) = (col.local.date(), col.local.time());
        let near: Vec<&Raid> = raids.iter().filter(|r| r.near(d)).collect();
        if near.is_empty() {
            out.push((None, "No raid then".into()));
            continue;
        }
        let during: Vec<&Raid> = near.iter().copied().filter(|r| r.during(d, t)).collect();
        let pool = if during.is_empty() { near } else { during };
        if pool.len() == 1 {
            out.push((Some(pool[0].row.id), String::new()));
            continue;
        }
        let exact: Vec<&Raid> = pool
            .iter()
            .copied()
            .filter(|r| r.local_date() == d)
            .collect();
        let pool = if exact.is_empty() { pool } else { exact };
        if pool.len() == 1 {
            out.push((Some(pool[0].row.id), String::new()));
            continue;
        }
        let here: Vec<&Player> = grid
            .players
            .iter()
            .filter(|p| p.present[ci] == Some(true))
            .collect();
        let scores: Vec<usize> = pool
            .iter()
            .map(|r| {
                here.iter()
                    .filter(|p| match_in_raid(r, &p.key).is_some())
                    .count()
            })
            .collect();
        let best = scores.iter().copied().max().unwrap_or(0);
        if best > 0 && scores.iter().filter(|s| **s == best).count() == 1 {
            let at = scores.iter().position(|s| *s == best).unwrap_or(0);
            out.push((Some(pool[at].row.id), String::new()));
            continue;
        }
        pending.push((ci, pool.iter().map(|r| r.row.id).collect()));
        out.push((None, format!("{} raids then: pick one", pool.len())));
    }
    for (ci, cands) in pending {
        let date = grid.columns[ci].local.date();
        let mut that_night: Vec<i64> = grid
            .columns
            .iter()
            .zip(out.iter())
            .filter(|(c, (e, _))| c.local.date() == date && e.is_some_and(|e| cands.contains(&e)))
            .filter_map(|(_, (e, _))| *e)
            .collect();
        that_night.sort_unstable();
        that_night.dedup();
        if that_night.len() == 1 {
            out[ci] = (Some(that_night[0]), String::new());
        }
    }
    out
}

/// Who a name is in a raid: their sign-up there, else a character on a
/// member's profile.
fn who(raid: Option<&Raid>, chars: &[MemberChar], key: &str) -> Option<Matched> {
    raid.and_then(|r| match_in_raid(r, key))
        .or_else(|| match_member(chars, key))
}

/// Pulls already imported for these raids: (event, boss key) → local times.
async fn existing_pulls(
    st: &AppState,
    event_ids: &[i64],
) -> AppResult<HashMap<(i64, String), Vec<NaiveDateTime>>> {
    let rows: Vec<(i64, String, NaiveDateTime)> = sqlx::query_as(
        "SELECT event_id, boss_key, pulled_local FROM attendance_pulls WHERE event_id = ANY($1)",
    )
    .bind(event_ids)
    .fetch_all(&st.db)
    .await?;
    let mut map: HashMap<(i64, String), Vec<NaiveDateTime>> = HashMap::new();
    for (e, k, t) in rows {
        map.entry((e, k)).or_default().push(t);
    }
    Ok(map)
}

fn is_duplicate(
    existing: &HashMap<(i64, String), Vec<NaiveDateTime>>,
    event_id: i64,
    col: &Column,
) -> bool {
    existing
        .get(&(event_id, col.boss_key.clone()))
        .is_some_and(|ts| {
            ts.iter()
                .any(|t| (*t - col.local).num_seconds().abs() <= SAME_PULL_SECS)
        })
}

struct Ctx {
    raids: Vec<Raid>,
    chars: Vec<MemberChar>,
    existing: HashMap<(i64, String), Vec<NaiveDateTime>>,
}

impl Ctx {
    fn raid(&self, id: i64) -> Option<&Raid> {
        self.raids.iter().find(|r| r.row.id == id)
    }
}

async fn context(
    st: &AppState,
    guild_id: i64,
    channels: &[i64],
    only: Option<i64>,
    grid: &Grid,
) -> AppResult<Ctx> {
    let first = grid.columns.iter().map(|c| c.local.date()).min();
    let last = grid.columns.iter().map(|c| c.local.date()).max();
    let (from, to) = match (first, last) {
        (Some(a), Some(b)) => (
            (a - Duration::days(2)).and_time(NaiveTime::MIN).and_utc(),
            (b + Duration::days(3)).and_time(NaiveTime::MIN).and_utc(),
        ),
        _ => (Utc::now(), Utc::now()),
    };
    let (raids, _) = raids_between(st, guild_id, channels, only, from, to).await?;
    let ids: Vec<i64> = raids.iter().map(|r| r.row.id).collect();
    let chars = member_chars(&st.db, guild_id).await?;
    let existing = existing_pulls(st, &ids).await?;
    Ok(Ctx {
        raids,
        chars,
        existing,
    })
}

// ---- Preview and import -----------------------------------------------------------

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct PreviewColumn {
    pub index: usize,
    pub line: usize,
    pub date: NaiveDate,
    /// "HH:MM".
    pub time: String,
    /// "2026-11-15T20:44:00" (the exporter's local time).
    pub local: NaiveDateTime,
    /// The boss as MRT wrote it (difficulty number off).
    pub boss_name: String,
    /// The encounter it matched ("" when it isn't on the lists).
    pub encounter_name: String,
    pub encounter_key: String,
    pub instance_name: String,
    pub difficulty: Option<i32>,
    pub difficulty_label: String,
    /// The raid it goes with (raid card: that raid; otherwise the best guess).
    #[serde(with = "option_as_string")]
    pub event_id: Option<i64>,
    /// Why it has no raid.
    pub reason: String,
    /// Raid card: on the raid's night (or the day either side).
    pub in_window: bool,
    /// Already imported for that raid (skipped).
    pub duplicate: bool,
    /// How many were there.
    pub present: usize,
}

#[derive(Serialize)]
pub struct PreviewPlayer {
    pub line: usize,
    pub name: String,
    pub key: String,
    /// Per column: true there, false not, null not in that grid.
    pub present: Vec<Option<bool>>,
    /// Who they are in each raid (event ID → match): their sign-up there,
    /// else a character on someone's profile. Missing: nobody.
    pub matches: HashMap<String, Matched>,
    /// Who they are without a raid (a character on someone's profile).
    pub member: Option<Matched>,
}

#[derive(Serialize)]
pub struct PreviewSignup {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    pub status: String,
    pub class: String,
    pub character_name: String,
    pub character_key: String,
}

#[derive(Serialize)]
pub struct PreviewRaid {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub local_date: NaiveDate,
    /// "20:00" where it's played.
    pub local_time: String,
    pub instance_name: String,
    pub game: String,
    pub signups: Vec<PreviewSignup>,
}

#[derive(Serialize)]
pub struct Preview {
    pub columns: Vec<PreviewColumn>,
    pub players: Vec<PreviewPlayer>,
    pub raids: Vec<PreviewRaid>,
    pub warnings: Vec<LineError>,
    /// Lines before the grid that were left out (a raid name, say).
    pub ignored: Vec<String>,
    pub duplicates: usize,
}

fn check_text(text: &str) -> AppResult<()> {
    if text.len() > MAX_TEXT {
        return Err(AppError::BadRequest(
            "That's too much to paste at once (500 KB). Paste a night or two at a time.".into(),
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
    let grid = parse_mrt(text).map_err(AppError::BadRequest)?;
    let ctx = context(st, guild_id, &channels, event, &grid).await?;
    let assigned: Vec<(Option<i64>, String)> = match event {
        Some(e) => grid
            .columns
            .iter()
            .map(|_| (Some(e), String::new()))
            .collect(),
        None => auto_assign(&ctx.raids, &grid),
    };
    let columns: Vec<PreviewColumn> = grid
        .columns
        .iter()
        .zip(assigned)
        .enumerate()
        .map(|(i, (c, (event_id, reason)))| {
            let raid = event_id.and_then(|e| ctx.raid(e));
            PreviewColumn {
                index: i,
                line: c.line,
                date: c.local.date(),
                time: c.local.format("%H:%M").to_string(),
                local: c.local,
                boss_name: c.boss.clone(),
                encounter_name: c
                    .encounter
                    .map(|(_, e)| e.name.to_string())
                    .unwrap_or_default(),
                encounter_key: c
                    .encounter
                    .map(|(k, e)| encounter_key(k, e))
                    .unwrap_or_default(),
                instance_name: c
                    .encounter
                    .map(|(k, _)| raidplan::instance_name(k))
                    .unwrap_or_default(),
                difficulty: c.difficulty,
                difficulty_label: c.difficulty.map(difficulty_label).unwrap_or("").to_string(),
                in_window: event.is_none() || raid.is_some_and(|r| r.near(c.local.date())),
                duplicate: event_id.is_some_and(|e| is_duplicate(&ctx.existing, e, c)),
                event_id,
                reason,
                present: grid
                    .players
                    .iter()
                    .filter(|p| p.present[i] == Some(true))
                    .count(),
            }
        })
        .collect();
    let players = grid
        .players
        .iter()
        .map(|p| PreviewPlayer {
            line: p.line,
            name: p.name.clone(),
            key: p.key.clone(),
            present: p.present.clone(),
            matches: ctx
                .raids
                .iter()
                .filter_map(|r| who(Some(r), &ctx.chars, &p.key).map(|m| (r.row.id.to_string(), m)))
                .collect(),
            member: match_member(&ctx.chars, &p.key),
        })
        .collect();
    let raids = ctx
        .raids
        .iter()
        .map(|r| PreviewRaid {
            id: r.row.id,
            channel_id: r.row.channel_id,
            title: r.row.title.clone(),
            starts_at: r.row.starts_at,
            local_date: r.local_date(),
            local_time: r.local_start.format("%H:%M").to_string(),
            instance_name: raidplan::instance_name(&r.row.loot_instance),
            game: r.row.game.clone(),
            signups: r
                .signups
                .iter()
                .map(|s| PreviewSignup {
                    user_id: s.user_id,
                    display_name: s.display_name.clone(),
                    status: s.status.clone(),
                    class: if s.character_class.is_empty() {
                        s.class.clone()
                    } else {
                        s.character_class.clone()
                    },
                    character_key: if s.character_name.is_empty() {
                        String::new()
                    } else {
                        char_key(&full(&s.character_name, &s.character_last_name))
                    },
                    character_name: full(&s.character_name, &s.character_last_name),
                })
                .collect(),
        })
        .collect();
    Ok(Preview {
        duplicates: columns.iter().filter(|c| c.duplicate).count(),
        columns,
        players,
        raids,
        warnings: grid.warnings,
        ignored: grid.ignored,
    })
}

fn full(name: &str, last: &str) -> String {
    if last.trim().is_empty() {
        name.to_string()
    } else {
        format!("{name} {last}")
    }
}

/// `POST /api/guilds/{id}/attendance/mrt/preview` `{text}` (raid managers).
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

/// `POST /api/events/{id}/attendance/mrt/preview` `{text}` (the raid's leaders).
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
    /// The column (as in the preview).
    pub index: usize,
    /// Its raid (ignored on a raid card's import: always that raid).
    #[serde(with = "option_as_string", default)]
    pub event_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub text: String,
    pub picks: Vec<ImportPick>,
}

#[derive(Serialize)]
pub struct ImportResult {
    #[serde(with = "option_as_string")]
    pub import_id: Option<i64>,
    /// Boss pulls added.
    pub bosses: usize,
    /// Characters there for at least one of them.
    pub players: usize,
    pub duplicates: usize,
    pub raids: usize,
    pub undo_until: Option<DateTime<Utc>>,
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
    let grid = parse_mrt(&req.text).map_err(AppError::BadRequest)?;
    let ctx = context(st, guild_id, &channels, event, &grid).await?;
    let mut picks: Vec<(usize, i64)> = Vec::new();
    for p in &req.picks {
        if p.index >= grid.columns.len() {
            return Err(AppError::BadRequest("That boss isn't in the paste.".into()));
        }
        let Some(e) = event.or(p.event_id) else {
            continue;
        };
        if ctx.raid(e).is_none() {
            return Err(AppError::BadRequest(
                "One of those raids isn't one you can import attendance into.".into(),
            ));
        }
        if !picks.iter().any(|(i, _)| *i == p.index) {
            picks.push((p.index, e));
        }
    }
    if picks.is_empty() {
        return Err(AppError::BadRequest(
            "Tick the bosses to import first.".into(),
        ));
    }
    let import_id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO attendance_imports (id, guild_id, event_id, imported_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(import_id)
    .bind(guild_id)
    .bind(event)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    let (mut bosses, mut duplicates) = (0usize, 0usize);
    let mut touched: Vec<i64> = Vec::new();
    let mut there: HashSet<String> = HashSet::new();
    let mut seen: Vec<(i64, String, NaiveDateTime)> = Vec::new();
    for (ci, event_id) in picks {
        let col = &grid.columns[ci];
        let raid = ctx.raid(event_id).ok_or(AppError::NotFound)?;
        let dup_in_paste = seen.iter().any(|(e, k, t)| {
            *e == event_id
                && *k == col.boss_key
                && (*t - col.local).num_seconds().abs() <= SAME_PULL_SECS
        });
        if dup_in_paste || is_duplicate(&ctx.existing, event_id, col) {
            duplicates += 1;
            continue;
        }
        seen.push((event_id, col.boss_key.clone(), col.local));
        let pull_id = st.ids.next_id();
        let done = sqlx::query(
            "INSERT INTO attendance_pulls (id, guild_id, event_id, import_id, boss_name, boss_key,
                 encounter_key, difficulty, pulled_local, pulled_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9 AT TIME ZONE $10)
             ON CONFLICT (event_id, boss_key, pulled_local) DO NOTHING",
        )
        .bind(pull_id)
        .bind(guild_id)
        .bind(event_id)
        .bind(import_id)
        .bind(&col.boss)
        .bind(&col.boss_key)
        .bind(col.encounter.map(|(k, e)| encounter_key(k, e)))
        .bind(col.difficulty)
        .bind(col.local)
        .bind(&raid.tz)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            duplicates += 1;
            continue;
        }
        bosses += 1;
        if !touched.contains(&event_id) {
            touched.push(event_id);
        }
        for p in &grid.players {
            let Some(present) = p.present[ci] else {
                continue;
            };
            let m = who(Some(raid), &ctx.chars, &p.key);
            if present {
                there.insert(p.key.clone());
            }
            sqlx::query(
                "INSERT INTO attendance_marks (pull_id, event_id, character_name, character_key,
                     character_class, user_id, present)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (pull_id, character_key) DO NOTHING",
            )
            .bind(pull_id)
            .bind(event_id)
            .bind(&p.name)
            .bind(&p.key)
            .bind(m.as_ref().map(|m| m.class.clone()).unwrap_or_default())
            .bind(m.as_ref().map(|m| m.user_id))
            .bind(present)
            .execute(&mut *tx)
            .await?;
        }
    }
    if bosses == 0 {
        tx.rollback().await?;
        return Ok(ImportResult {
            import_id: None,
            bosses: 0,
            players: 0,
            duplicates,
            raids: 0,
            undo_until: None,
        });
    }
    sqlx::query("UPDATE attendance_imports SET boss_count = $2, player_count = $3 WHERE id = $1")
        .bind(import_id)
        .bind(bosses as i32)
        .bind(there.len() as i32)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut details = format!(
        "Imported attendance from MRT: {bosses} boss{} · {} there",
        if bosses == 1 { "" } else { "es" },
        there.len()
    );
    let names: Vec<String> = touched
        .iter()
        .filter_map(|id| ctx.raid(*id))
        .map(|r| format!("{} ({})", r.row.title, r.local_date().format("%-d %b")))
        .collect();
    if !names.is_empty() {
        details.push_str(&format!(" · {}", names.join(", ")));
    }
    if duplicates > 0 {
        details.push_str(&format!(" · {duplicates} already in"));
    }
    crate::audit::Entry::new(guild_id, user_id, "attendance_import")
        .details(details)
        .save(st)
        .await;
    for e in &touched {
        broadcast(st, *e).await?;
    }
    Ok(ImportResult {
        import_id: Some(import_id),
        bosses,
        players: there.len(),
        duplicates,
        raids: touched.len(),
        undo_until: Some(Utc::now() + Duration::hours(UNDO_HOURS)),
    })
}

/// `POST /api/guilds/{id}/attendance/mrt/import` `{text, picks: [{index, event_id}]}`.
pub async fn guild_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportResult>> {
    Ok(Json(import(&st, user.id, Some(guild_id), None, req).await?))
}

/// `POST /api/events/{id}/attendance/mrt/import` `{text, picks: [{index}]}`.
pub async fn raid_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportResult>> {
    Ok(Json(import(&st, user.id, None, Some(event_id), req).await?))
}

// ---- Imports: list, undo, delete -------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
pub struct ImportInfo {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "option_as_string")]
    pub event_id: Option<i64>,
    #[serde(with = "option_as_string")]
    pub imported_by: Option<i64>,
    pub imported_by_name: Option<String>,
    pub boss_count: i32,
    pub player_count: i32,
    /// Bosses still in.
    pub remaining: i64,
    /// The raids it went to ("BT Saturday", …).
    pub raids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub undone_at: Option<DateTime<Utc>>,
    #[sqlx(skip)]
    pub can_undo: bool,
    #[sqlx(skip)]
    pub can_delete: bool,
}

/// `GET /api/guilds/{id}/attendance/mrt/imports` (raid managers).
pub async fn list_imports(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<ImportInfo>>> {
    super::loot::outhouse_loot_manager(&st, guild_id, user.id).await?;
    let mut rows = sqlx::query_as::<_, ImportInfo>(
        "SELECT i.id, i.event_id, i.imported_by,
                COALESCE(gm.nickname, u.display_name) AS imported_by_name, i.boss_count,
                i.player_count,
                (SELECT count(*) FROM attendance_pulls p WHERE p.import_id = i.id) AS remaining,
                ARRAY(SELECT DISTINCT e.title FROM attendance_pulls p JOIN events e ON e.id = p.event_id
                      WHERE p.import_id = i.id) AS raids,
                i.created_at, i.undone_at
         FROM attendance_imports i
         LEFT JOIN users u ON u.id = i.imported_by
         LEFT JOIN guild_members gm ON gm.guild_id = i.guild_id AND gm.user_id = i.imported_by
         WHERE i.guild_id = $1
         ORDER BY i.created_at DESC LIMIT 20",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let cutoff = Utc::now() - Duration::hours(UNDO_HOURS);
    for r in &mut rows {
        r.can_undo =
            r.imported_by == Some(user.id) && r.undone_at.is_none() && r.created_at > cutoff;
        r.can_delete = r.undone_at.is_none() && r.remaining > 0;
    }
    Ok(Json(rows))
}

/// `DELETE /api/attendance/imports/{id}`: takes a whole import back out.
/// Its importer can undo it for 24 hours; raid managers (Manage Events or a
/// raid manager role) can delete it any time. Logged.
pub async fn delete_import(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(import_id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    #[allow(clippy::type_complexity)]
    let found: Option<(i64, Option<i64>, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT guild_id, imported_by, created_at, undone_at FROM attendance_imports WHERE id = $1",
    )
    .bind(import_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((guild_id, by, at, undone)) = found else {
        return Err(AppError::NotFound);
    };
    member_ctx(&st.db, guild_id, user.id).await?;
    let manager = super::loot::outhouse_loot_manager(&st, guild_id, user.id)
        .await
        .is_ok();
    let in_time = at > Utc::now() - Duration::hours(UNDO_HOURS);
    let undo = by == Some(user.id) && in_time;
    if !undo && !manager {
        return Err(if by == Some(user.id) {
            AppError::BadRequest(
                "Imports can be undone for 24 hours. After that, a raid manager can delete it."
                    .into(),
            )
        } else {
            AppError::Forbidden
        });
    }
    if undone.is_some() {
        return Err(AppError::BadRequest(
            "That import was already taken out.".into(),
        ));
    }
    let mut tx = st.db.begin().await?;
    let events: Vec<i64> =
        sqlx::query_scalar("DELETE FROM attendance_pulls WHERE import_id = $1 RETURNING event_id")
            .bind(import_id)
            .fetch_all(&mut *tx)
            .await?;
    sqlx::query("UPDATE attendance_imports SET undone_at = now() WHERE id = $1")
        .bind(import_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let action = if undo {
        "attendance_import_undo"
    } else {
        "attendance_import_delete"
    };
    crate::audit::Entry::new(guild_id, user.id, action)
        .details(format!(
            "{} an MRT attendance import ({} boss{})",
            if undo { "Undid" } else { "Deleted" },
            events.len(),
            if events.len() == 1 { "" } else { "es" }
        ))
        .save(&st)
        .await;
    let mut touched = events.clone();
    touched.sort_unstable();
    touched.dedup();
    for e in touched {
        let _ = broadcast(&st, e).await;
    }
    Ok(Json(serde_json::json!({ "removed": events.len() })))
}

// ---- A raid's attendance (its card) ------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
pub struct PullInfo {
    #[serde(with = "as_string")]
    pub id: i64,
    pub boss_name: String,
    pub encounter_key: Option<String>,
    pub difficulty: Option<i32>,
    pub pulled_at: DateTime<Utc>,
    pub pulled_local: NaiveDateTime,
    #[serde(with = "as_string")]
    pub import_id: i64,
    #[sqlx(skip)]
    pub encounter_name: String,
}

#[derive(Serialize)]
pub struct RaidPlayer {
    pub character_name: String,
    pub character_key: String,
    pub character_class: String,
    #[serde(with = "option_as_string")]
    pub user_id: Option<i64>,
    pub display_name: Option<String>,
    /// Per pull (same order).
    pub present: Vec<bool>,
}

#[derive(Serialize)]
pub struct RaidAttendance {
    pub pulls: Vec<PullInfo>,
    pub players: Vec<RaidPlayer>,
    /// Members marked by hand (they override what the import says).
    pub manual: Vec<super::attendance::AttendanceRecord>,
    /// You can edit it (a raid leader, once the raid has started).
    pub can_edit: bool,
}

/// `GET /api/events/{id}/attendance/mrt`: the raid's attendance: imported
/// bosses and the hand marks (anyone who can see the raid).
pub async fn raid_attendance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<RaidAttendance>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let can_edit = row.starts_at <= Utc::now() && can_manage(&st.db, &row, &access).await?;
    let manual = super::attendance::records(&st, event_id).await?;
    let mut pulls = sqlx::query_as::<_, PullInfo>(
        "SELECT id, boss_name, encounter_key, difficulty, pulled_at, pulled_local, import_id
         FROM attendance_pulls WHERE event_id = $1 ORDER BY pulled_at, id",
    )
    .bind(event_id)
    .fetch_all(&st.db)
    .await?;
    for p in &mut pulls {
        p.encounter_name = find_encounter(&p.boss_name)
            .map(|(_, e)| e.name.to_string())
            .unwrap_or_default();
    }
    let at: HashMap<i64, usize> = pulls.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    #[derive(sqlx::FromRow)]
    struct Mark {
        pull_id: i64,
        character_name: String,
        character_key: String,
        character_class: String,
        user_id: Option<i64>,
        display_name: Option<String>,
        present: bool,
    }
    let marks = sqlx::query_as::<_, Mark>(
        "SELECT m.pull_id, m.character_name, m.character_key, m.character_class, m.user_id,
                COALESCE(gm.nickname, u.display_name) AS display_name, m.present
         FROM attendance_marks m
         LEFT JOIN users u ON u.id = m.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = $2 AND gm.user_id = m.user_id
         WHERE m.event_id = $1
         ORDER BY lower(m.character_name)",
    )
    .bind(event_id)
    .bind(row.guild_id)
    .fetch_all(&st.db)
    .await?;
    let mut order: Vec<(String, Option<i64>)> = Vec::new();
    let mut players: HashMap<(String, Option<i64>), RaidPlayer> = HashMap::new();
    for m in marks {
        let k = (m.character_key.clone(), m.user_id);
        let p = players.entry(k.clone()).or_insert_with(|| {
            order.push(k.clone());
            RaidPlayer {
                character_name: m.character_name.clone(),
                character_key: m.character_key.clone(),
                character_class: m.character_class.clone(),
                user_id: m.user_id,
                display_name: m.display_name.clone(),
                present: vec![false; pulls.len()],
            }
        });
        if let Some(&i) = at.get(&m.pull_id) {
            p.present[i] = p.present[i] || m.present;
        }
        if p.character_class.is_empty() {
            p.character_class = m.character_class;
        }
    }
    Ok(Json(RaidAttendance {
        pulls,
        players: order
            .into_iter()
            .filter_map(|k| players.remove(&k))
            .collect(),
        manual,
        can_edit,
    }))
}

// ---- Counting ------------------------------------------------------------------------

/// A raid with attendance (imported or marked by hand).
#[derive(sqlx::FromRow, Clone)]
struct CountedRaid {
    id: i64,
    loot_instance: String,
    local_date: NaiveDate,
}

#[derive(sqlx::FromRow)]
struct MarkSum {
    event_id: i64,
    character_key: String,
    character_name: String,
    character_class: String,
    user_id: Option<i64>,
    present: bool,
}

#[derive(sqlx::FromRow)]
struct SignupSum {
    event_id: i64,
    user_id: i64,
    character_name: String,
    character_last_name: String,
    class: String,
    character_class: String,
}

#[derive(Serialize, Clone, Default)]
pub struct CharacterAttendance {
    pub character_key: String,
    pub character_name: String,
    pub character_class: String,
    #[serde(with = "option_as_string")]
    pub user_id: Option<i64>,
    pub display_name: Option<String>,
    /// Raids they were there for (at least one boss).
    pub attended: i64,
    /// Raids they attended or no-showed.
    pub eligible: i64,
    /// attended / eligible, rounded (100 when there's nothing to count).
    pub percent: i64,
    /// Signed up (yes, late or tentative), didn't come (or marked absent).
    pub no_shows: i64,
    pub last_attended: Option<NaiveDate>,
}

/// What counts for a member on a raid with attendance: the
/// `raid_attendance_outcomes` view (migration 0047), where a hand mark
/// overrides the import. `outcome`: attended, no_show, excused, or absent
/// (marked absent without signing up: not counted).
#[derive(sqlx::FromRow, Clone)]
struct OutcomeRow {
    event_id: i64,
    user_id: i64,
    outcome: String,
    manual_status: Option<String>,
}

async fn outcomes(
    st: &AppState,
    ids: &[i64],
    only_user: Option<i64>,
) -> AppResult<Vec<OutcomeRow>> {
    Ok(sqlx::query_as::<_, OutcomeRow>(
        "SELECT event_id, user_id, outcome, manual_status FROM raid_attendance_outcomes
         WHERE event_id = ANY($1) AND ($2::BIGINT IS NULL OR user_id = $2)",
    )
    .bind(ids)
    .bind(only_user)
    .fetch_all(&st.db)
    .await?)
}

/// Per character over these raids: attended, no-shows, %. A member's raid
/// goes on the characters MRT saw them on; otherwise (marked by hand, or a
/// no-show) on the character they signed up with, else the one they raid on
/// most, else their name.
async fn count(
    st: &AppState,
    guild_id: i64,
    raids: &[CountedRaid],
    only_user: Option<i64>,
) -> AppResult<Vec<CharacterAttendance>> {
    let ids: Vec<i64> = raids.iter().map(|r| r.id).collect();
    let date: HashMap<i64, NaiveDate> = raids.iter().map(|r| (r.id, r.local_date)).collect();
    let marks = sqlx::query_as::<_, MarkSum>(
        "SELECT event_id, character_key, max(character_name) AS character_name,
                max(character_class) AS character_class, user_id, bool_or(present) AS present
         FROM attendance_marks WHERE event_id = ANY($1)
           AND ($2::BIGINT IS NULL OR user_id = $2)
         GROUP BY event_id, character_key, user_id",
    )
    .bind(&ids)
    .bind(only_user)
    .fetch_all(&st.db)
    .await?;
    let signups = sqlx::query_as::<_, SignupSum>(
        "SELECT event_id, user_id, character_name, character_last_name, class, character_class
         FROM event_signups WHERE event_id = ANY($1) AND ($2::BIGINT IS NULL OR user_id = $2)",
    )
    .bind(&ids)
    .bind(only_user)
    .fetch_all(&st.db)
    .await?;
    let names: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
        "SELECT u.id, COALESCE(gm.nickname, u.display_name)
         FROM users u LEFT JOIN guild_members gm ON gm.guild_id = $1 AND gm.user_id = u.id
         WHERE u.id IN (SELECT user_id FROM guild_members WHERE guild_id = $1)
            OR u.id IN (SELECT user_id FROM event_attendance WHERE event_id = ANY($2))",
    )
    .bind(guild_id)
    .bind(&ids)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    let outcomes = outcomes(st, &ids, only_user).await?;

    type Key = (String, Option<i64>);
    let mut rows: HashMap<Key, CharacterAttendance> = HashMap::new();
    let mut attended: HashMap<Key, HashSet<i64>> = HashMap::new();
    let mut missed: HashMap<Key, HashSet<i64>> = HashMap::new();
    // The characters MRT saw each member on, per raid.
    let mut seen_on: HashMap<(i64, i64), Vec<Key>> = HashMap::new();
    // Raids MRT saw each character at (a member's usual character).
    let mut usual: HashMap<Key, usize> = HashMap::new();
    for m in &marks {
        let k = (m.character_key.clone(), m.user_id);
        let r = rows
            .entry(k.clone())
            .or_insert_with(|| CharacterAttendance {
                character_key: m.character_key.clone(),
                character_name: m.character_name.clone(),
                character_class: m.character_class.clone(),
                user_id: m.user_id,
                display_name: m.user_id.and_then(|u| names.get(&u).cloned()),
                ..Default::default()
            });
        if r.character_class.is_empty() {
            r.character_class = m.character_class.clone();
        }
        if !m.present {
            continue;
        }
        match m.user_id {
            // Nobody here by that name: counted as MRT says.
            None => {
                attended.entry(k).or_default().insert(m.event_id);
            }
            Some(u) => {
                seen_on.entry((u, m.event_id)).or_default().push(k.clone());
                *usual.entry(k).or_default() += 1;
            }
        }
    }
    let signup_of: HashMap<(i64, i64), &SignupSum> = signups
        .iter()
        .map(|s| ((s.user_id, s.event_id), s))
        .collect();
    for o in &outcomes {
        let came = match o.outcome.as_str() {
            "attended" => true,
            "no_show" => false,
            _ => continue,
        };
        let seen = if came {
            seen_on
                .get(&(o.user_id, o.event_id))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let keys = if seen.is_empty() {
            let s = signup_of.get(&(o.user_id, o.event_id)).copied();
            let signed = s
                .filter(|s| !s.character_name.is_empty())
                .map(|s| char_key(&full(&s.character_name, &s.character_last_name)));
            let key = signed.unwrap_or_else(|| {
                usual
                    .iter()
                    .filter(|((_, u), _)| *u == Some(o.user_id))
                    .max_by(|a, b| a.1.cmp(b.1).then(b.0 .0.cmp(&a.0 .0)))
                    .map(|((c, _), _)| c.clone())
                    .unwrap_or_else(|| char_key(names.get(&o.user_id).map_or("", |n| n.as_str())))
            });
            let k = (key, Some(o.user_id));
            let class = s
                .map(|s| {
                    if s.character_class.is_empty() {
                        s.class.clone()
                    } else {
                        s.character_class.clone()
                    }
                })
                .unwrap_or_default();
            let r = rows
                .entry(k.clone())
                .or_insert_with(|| CharacterAttendance {
                    character_key: k.0.clone(),
                    character_name: match s {
                        Some(s) if !s.character_name.is_empty() => {
                            full(&s.character_name, &s.character_last_name)
                        }
                        _ => names.get(&o.user_id).cloned().unwrap_or_default(),
                    },
                    character_class: class.clone(),
                    user_id: Some(o.user_id),
                    display_name: names.get(&o.user_id).cloned(),
                    ..Default::default()
                });
            if r.character_class.is_empty() {
                r.character_class = class;
            }
            vec![k]
        } else {
            seen
        };
        let into = if came { &mut attended } else { &mut missed };
        for k in keys {
            into.entry(k).or_default().insert(o.event_id);
        }
    }
    let mut out: Vec<CharacterAttendance> = rows
        .into_iter()
        .map(|(k, mut r)| {
            let there = attended.get(&k).cloned().unwrap_or_default();
            let no_shows = missed.get(&k).map_or(0, |m| m.difference(&there).count()) as i64;
            r.attended = there.len() as i64;
            r.no_shows = no_shows;
            r.eligible = r.attended + no_shows;
            r.percent = percent(r.attended, r.eligible);
            r.last_attended = there.iter().filter_map(|e| date.get(e)).max().copied();
            r
        })
        // Listed but never counted (not there, excused, never signed up).
        .filter(|r| r.eligible > 0)
        .collect();
    out.sort_by(|a, b| {
        b.attended
            .cmp(&a.attended)
            .then(b.percent.cmp(&a.percent))
            .then(
                a.character_name
                    .to_lowercase()
                    .cmp(&b.character_name.to_lowercase()),
            )
    });
    Ok(out)
}

/// attended / eligible, rounded (100 when there's nothing to count).
fn percent(attended: i64, eligible: i64) -> i64 {
    if eligible == 0 {
        100
    } else {
        ((attended as f64 * 100.0) / eligible as f64).round() as i64
    }
}

/// A member's raids in total (whichever character): attended, no-shows,
/// excused, %.
#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Totals {
    pub attended: i64,
    pub no_shows: i64,
    pub excused: i64,
    pub eligible: i64,
    pub percent: i64,
}

fn totals<'a>(rows: impl IntoIterator<Item = &'a OutcomeRow>) -> Totals {
    let mut t = Totals::default();
    for o in rows {
        match o.outcome.as_str() {
            "attended" => t.attended += 1,
            "no_show" => t.no_shows += 1,
            "excused" => t.excused += 1,
            _ => {}
        }
    }
    t.eligible = t.attended + t.no_shows;
    t.percent = percent(t.attended, t.eligible);
    t
}

/// The outhouse's raids with attendance (imported or marked by hand) in
/// these channels.
async fn counted_raids(
    st: &AppState,
    guild_id: i64,
    channels: &[i64],
    tz: &str,
) -> AppResult<Vec<CountedRaid>> {
    Ok(sqlx::query_as::<_, CountedRaid>(
        "SELECT e.id, e.loot_instance, (e.starts_at AT TIME ZONE $3)::DATE AS local_date
         FROM events e
         WHERE e.guild_id = $1 AND e.channel_id = ANY($2)
           AND (EXISTS (SELECT 1 FROM attendance_pulls p WHERE p.event_id = e.id)
                OR EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id))
         ORDER BY e.starts_at",
    )
    .bind(guild_id)
    .bind(channels)
    .bind(tz)
    .fetch_all(&st.db)
    .await?)
}

async fn outhouse_tz(st: &AppState, guild_id: i64) -> AppResult<String> {
    let settings = super::raid_settings::load(st, guild_id).await?;
    Ok(settings
        .realm_effective_time_zone
        .clone()
        .filter(|z| !z.is_empty())
        .unwrap_or_else(|| "UTC".into()))
}

#[derive(Deserialize, Default)]
pub struct RangeQuery {
    /// "4w", "8w" (default), "phase", "all" or "custom" (with from/to).
    range: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Serialize)]
pub struct Range {
    pub range: String,
    /// "last 4 weeks", "Phase 3", "1 Oct – 15 Nov", "all time".
    pub label: String,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// The raids in a range, and the range as it was understood.
fn in_range(
    raids: Vec<CountedRaid>,
    q: &RangeQuery,
    today: NaiveDate,
) -> AppResult<(Vec<CountedRaid>, Range)> {
    let day = |v: &Option<String>| -> AppResult<Option<NaiveDate>> {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| AppError::BadRequest("Dates are YYYY-MM-DD.".into())),
        }
    };
    let weeks = |n: i64, label: &str, raids: Vec<CountedRaid>| {
        let from = today - Duration::weeks(n);
        let raids = raids
            .into_iter()
            .filter(|r| r.local_date >= from && r.local_date <= today)
            .collect();
        (
            raids,
            Range {
                range: format!("{n}w"),
                label: label.to_string(),
                from: Some(from),
                to: Some(today),
            },
        )
    };
    let range = q.range.as_deref().unwrap_or("8w");
    Ok(match range {
        "4w" => weeks(4, "last 4 weeks", raids),
        "all" => {
            let from = raids.iter().map(|r| r.local_date).min();
            (
                raids,
                Range {
                    range: "all".into(),
                    label: "all time".into(),
                    from,
                    to: Some(today),
                },
            )
        }
        "phase" => {
            // The phase of the latest listed raid with attendance.
            let latest = raids.iter().rev().find_map(|r| phase_of(&r.loot_instance));
            match latest {
                None => {
                    let (r, mut range) = weeks(8, "last 8 weeks", raids);
                    range.label = "last 8 weeks (no listed raids for a phase)".into();
                    (r, range)
                }
                Some(ph) => {
                    let mine: Vec<CountedRaid> = raids
                        .into_iter()
                        .filter(|r| phase_of(&r.loot_instance) == Some(ph))
                        .collect();
                    let from = mine.iter().map(|r| r.local_date).min();
                    (
                        mine,
                        Range {
                            range: "phase".into(),
                            label: format!("Phase {}", ph.1),
                            from,
                            to: Some(today),
                        },
                    )
                }
            }
        }
        "custom" => {
            let (from, to) = (day(&q.from)?, day(&q.to)?);
            let raids = raids
                .into_iter()
                .filter(|r| {
                    from.is_none_or(|f| r.local_date >= f) && to.is_none_or(|t| r.local_date <= t)
                })
                .collect();
            let fmt = |d: Option<NaiveDate>| d.map(|d| d.format("%-d %b %Y").to_string());
            let label = match (fmt(from), fmt(to)) {
                (Some(a), Some(b)) => format!("{a} – {b}"),
                (Some(a), None) => format!("since {a}"),
                (None, Some(b)) => format!("up to {b}"),
                (None, None) => "all time".into(),
            };
            (
                raids,
                Range {
                    range: "custom".into(),
                    label,
                    from,
                    to,
                },
            )
        }
        _ => weeks(8, "last 8 weeks", raids),
    })
}

#[derive(Serialize)]
pub struct OuthouseAttendance {
    #[serde(flatten)]
    pub range: Range,
    /// Raids with attendance (imported or marked by hand) in the range.
    pub raids: usize,
    pub characters: Vec<CharacterAttendance>,
}

/// `GET /api/guilds/{id}/attendance/mrt/characters?range=4w|8w|phase|all|custom&from=&to=`:
/// attendance per character (raids in channels you can see).
pub async fn guild_characters(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<OuthouseAttendance>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let (channels, _) = history_scope(&st, &ctx).await?;
    let tz = outhouse_tz(&st, guild_id).await?;
    let all = counted_raids(&st, guild_id, &channels, &tz).await?;
    let today = local_today(&st, &tz).await?;
    let (raids, range) = in_range(all, &q, today)?;
    let characters = count(&st, guild_id, &raids, None).await?;
    Ok(Json(OuthouseAttendance {
        range,
        raids: raids.len(),
        characters,
    }))
}

async fn local_today(st: &AppState, tz: &str) -> AppResult<NaiveDate> {
    Ok(sqlx::query_scalar("SELECT (now() AT TIME ZONE $1)::DATE")
        .bind(tz)
        .fetch_one(&st.db)
        .await?)
}

#[derive(Serialize)]
pub struct ProfileCharacter {
    pub character_key: String,
    pub character_name: String,
    pub character_class: String,
    /// The last 8 weeks.
    pub recent: CharacterAttendance,
    /// Every raid with attendance.
    pub all_time: CharacterAttendance,
}

#[derive(Serialize)]
pub struct ProfileAttendance {
    #[serde(with = "as_string")]
    pub guild_id: i64,
    pub guild_name: String,
    pub characters: Vec<ProfileCharacter>,
    /// Whichever character: the last 8 weeks, and every raid with attendance.
    pub recent: Totals,
    pub all_time: Totals,
    /// Their last raids with attendance, newest first (up to 10).
    pub raids: Vec<ProfileRaid>,
}

#[derive(Serialize)]
pub struct ProfileRaid {
    #[serde(with = "as_string")]
    pub event_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    /// attended, no_show, excused or absent.
    pub outcome: String,
    /// Their hand mark, if they have one (present, late, benched, …).
    pub manual_status: Option<String>,
}

/// `GET /api/users/{id}/attendance`: their attendance by character in the
/// outhouses you're both in (raids in channels you can see).
pub async fn user_attendance(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(target): Path<i64>,
) -> AppResult<Json<Vec<ProfileAttendance>>> {
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
    let mut out = Vec::new();
    for (guild_id, guild_name) in shared {
        let ctx = member_ctx(&st.db, guild_id, user.id).await?;
        let mut channels =
            visible_text_channels(&st.db, &ctx, outhouse_common::permissions::VIEW_CHANNEL).await?;
        drop_age_gated(&st.db, user.id, &mut channels).await?;
        let tz = outhouse_tz(&st, guild_id).await?;
        let all = counted_raids(&st, guild_id, &channels, &tz).await?;
        if all.is_empty() {
            continue;
        }
        let today = local_today(&st, &tz).await?;
        let (recent_raids, _) = in_range(all.clone(), &RangeQuery::default(), today)?;
        let all_time = count(&st, guild_id, &all, Some(target)).await?;
        let recent = count(&st, guild_id, &recent_raids, Some(target)).await?;
        let characters: Vec<ProfileCharacter> = all_time
            .into_iter()
            .map(|a| {
                let r = recent
                    .iter()
                    .find(|r| r.character_key == a.character_key)
                    .cloned()
                    .unwrap_or_else(|| CharacterAttendance {
                        character_key: a.character_key.clone(),
                        character_name: a.character_name.clone(),
                        character_class: a.character_class.clone(),
                        user_id: a.user_id,
                        display_name: a.display_name.clone(),
                        percent: 100,
                        ..Default::default()
                    });
                ProfileCharacter {
                    character_key: a.character_key.clone(),
                    character_name: a.character_name.clone(),
                    character_class: a.character_class.clone(),
                    recent: r,
                    all_time: a,
                }
            })
            .collect();
        let ids: Vec<i64> = all.iter().map(|r| r.id).collect();
        let theirs = outcomes(&st, &ids, Some(target)).await?;
        let recent_ids: HashSet<i64> = recent_raids.iter().map(|r| r.id).collect();
        let recent_totals = totals(theirs.iter().filter(|o| recent_ids.contains(&o.event_id)));
        let all_totals = totals(&theirs);
        let by_event: HashMap<i64, &OutcomeRow> = theirs.iter().map(|o| (o.event_id, o)).collect();
        let info: Vec<(i64, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, title, starts_at FROM events WHERE id = ANY($1)
             ORDER BY starts_at DESC, id DESC",
        )
        .bind(by_event.keys().copied().collect::<Vec<i64>>())
        .fetch_all(&st.db)
        .await?;
        let raids: Vec<ProfileRaid> = info
            .into_iter()
            .take(10)
            .filter_map(|(id, title, starts_at)| {
                by_event.get(&id).map(|o| ProfileRaid {
                    event_id: id,
                    title,
                    starts_at,
                    outcome: o.outcome.clone(),
                    manual_status: o.manual_status.clone(),
                })
            })
            .collect();
        if !characters.is_empty() || !raids.is_empty() {
            out.push(ProfileAttendance {
                guild_id,
                guild_name,
                characters,
                recent: recent_totals,
                all_time: all_totals,
                raids,
            });
        }
    }
    Ok(Json(out))
}

// ---- The outhouse's attendance table (the "Raid attendance" window) ------------------

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct TableRaid {
    #[serde(with = "as_string")]
    pub id: i64,
    #[serde(with = "as_string")]
    pub channel_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
pub struct TableMember {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub display_name: String,
    pub present: i64,
    pub late: i64,
    pub benched: i64,
    pub excused: i64,
    /// No-shows (signed up and not there, or marked absent).
    pub absent: i64,
    /// Raids that count: attended + no-shows.
    pub total: i64,
    /// Attended out of `total` (100 when nothing counts).
    pub percent: i64,
    /// Their cell in each of `raids` (same order): present, late, benched,
    /// excused, absent (a no-show, or marked absent) or "" (not there).
    pub history: Vec<String>,
}

#[derive(Serialize)]
pub struct AttendanceTable {
    /// Newest first.
    pub raids: Vec<TableRaid>,
    pub members: Vec<TableMember>,
}

/// One member's cell on a raid. Hand marks show as marked; otherwise MRT
/// presence is "present", a no-show "absent", and a bench sign-up "benched"
/// (shown, never counted).
fn table_cell(o: Option<&OutcomeRow>, bench: bool) -> &'static str {
    match o {
        Some(o) => match (o.manual_status.as_deref(), o.outcome.as_str()) {
            (Some("present"), _) => "present",
            (Some("late"), _) => "late",
            (Some("benched"), _) => "benched",
            (Some("excused"), _) => "excused",
            (Some("absent"), _) | (_, "no_show") => "absent",
            (_, "attended") => "present",
            _ if bench => "benched",
            _ => "",
        },
        None if bench => "benched",
        None => "",
    }
}

/// `GET /api/guilds/{id}/attendance?limit=N`: everyone's attendance over
/// the last N raids with attendance (MRT imports or hand marks; default 10,
/// up to 50), counted like everywhere else.
pub async fn guild_table(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> AppResult<Json<AttendanceTable>> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let (channels, _) = history_scope(&st, &ctx).await?;
    let raids: Vec<(i64, i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT e.id, e.channel_id, e.title, e.starts_at FROM events e
         WHERE e.guild_id = $1 AND e.channel_id = ANY($2)
           AND (EXISTS (SELECT 1 FROM attendance_pulls p WHERE p.event_id = e.id)
                OR EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id))
         ORDER BY e.starts_at DESC, e.id DESC
         LIMIT $3",
    )
    .bind(guild_id)
    .bind(&channels)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;
    let ids: Vec<i64> = raids.iter().map(|r| r.0).collect();
    let column: HashMap<i64, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let rows = outcomes(&st, &ids, None).await?;
    let bench: HashSet<(i64, i64)> = sqlx::query_as::<_, (i64, i64)>(
        "SELECT event_id, user_id FROM event_signups WHERE event_id = ANY($1) AND status = 'bench'",
    )
    .bind(&ids)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    let mut by: HashMap<(i64, i64), &OutcomeRow> = HashMap::new();
    let mut people: HashSet<i64> = HashSet::new();
    for o in &rows {
        by.insert((o.event_id, o.user_id), o);
        people.insert(o.user_id);
    }
    for (_, u) in &bench {
        people.insert(*u);
    }
    let ids_people: Vec<i64> = people.iter().copied().collect();
    let names: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
        "SELECT u.id, COALESCE(gm.nickname, u.display_name) FROM users u
         LEFT JOIN guild_members gm ON gm.guild_id = $2 AND gm.user_id = u.id
         WHERE u.id = ANY($1)",
    )
    .bind(&ids_people)
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    let mut members: Vec<TableMember> = Vec::new();
    for u in ids_people {
        let mut m = TableMember {
            user_id: u,
            display_name: names.get(&u).cloned().unwrap_or_default(),
            history: vec![String::new(); ids.len()],
            ..Default::default()
        };
        let mine: Vec<&OutcomeRow> = rows.iter().filter(|o| o.user_id == u).collect();
        for &e in &ids {
            let o = by.get(&(e, u)).copied();
            let cell = table_cell(o, bench.contains(&(e, u)));
            if let Some(&i) = column.get(&e) {
                m.history[i] = cell.to_string();
            }
            match cell {
                "present" => m.present += 1,
                "late" => m.late += 1,
                "benched" => m.benched += 1,
                "excused" => m.excused += 1,
                _ => {}
            }
        }
        let t = totals(mine.into_iter());
        m.absent = t.no_shows;
        m.total = t.eligible;
        m.percent = t.percent;
        if m.history.iter().any(|h| !h.is_empty()) {
            members.push(m);
        }
    }
    members.sort_by(|a, b| {
        b.percent.cmp(&a.percent).then(b.total.cmp(&a.total)).then(
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase()),
        )
    });
    Ok(Json(AttendanceTable {
        raids: raids
            .into_iter()
            .map(|(id, channel_id, title, starts_at)| TableRaid {
                id,
                channel_id,
                title,
                starts_at,
            })
            .collect(),
        members,
    }))
}

// ---- The no-show hint on sign-ups (raid leaders) ----------------------------------------

#[derive(Serialize)]
pub struct Reliability {
    #[serde(with = "as_string")]
    pub user_id: i64,
    pub no_shows: i64,
    /// Raids with attendance they signed up to in the window.
    pub signed_up: i64,
}

#[derive(Serialize)]
pub struct ReliabilityReply {
    pub days: i64,
    /// Only people with at least `HINT_NO_SHOWS` no-shows.
    pub people: Vec<Reliability>,
}

/// `GET /api/events/{id}/attendance/reliability` (the raid's leaders): the
/// people signed up to this raid with 2 or more no-shows in the 4 weeks up
/// to it (raids with attendance, imported or marked by hand, in channels the
/// leader can see; a hand mark overrides the import).
pub async fn reliability(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<ReliabilityReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    let ctx = member_ctx(&st.db, row.guild_id, user.id).await?;
    let channels =
        visible_text_channels(&st.db, &ctx, outhouse_common::permissions::VIEW_CHANNEL).await?;
    let until = row.starts_at.min(Utc::now());
    let since = until - Duration::days(HINT_DAYS);
    let people: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH raids AS (
             SELECT e.id FROM events e
             WHERE e.guild_id = $1 AND e.channel_id = ANY($2)
               AND e.starts_at > $3 AND e.starts_at <= $4
               AND (EXISTS (SELECT 1 FROM attendance_pulls p WHERE p.event_id = e.id)
                    OR EXISTS (SELECT 1 FROM event_attendance a WHERE a.event_id = e.id))
         )
         SELECT o.user_id,
                count(*) FILTER (WHERE o.outcome = 'no_show') AS no_shows,
                count(*) FILTER (WHERE o.signup_status IN ('yes', 'late', 'tentative')) AS signed_up
         FROM raid_attendance_outcomes o
         WHERE o.event_id IN (SELECT id FROM raids)
           AND o.user_id IN (SELECT user_id FROM event_signups WHERE event_id = $5)
         GROUP BY o.user_id",
    )
    .bind(row.guild_id)
    .bind(&channels)
    .bind(since)
    .bind(until)
    .bind(event_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(ReliabilityReply {
        days: HINT_DAYS,
        people: people
            .into_iter()
            .filter(|p| p.1 >= HINT_NO_SHOWS)
            .map(|(user_id, no_shows, signed_up)| Reliability {
                user_id,
                no_shows,
                signed_up,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_count_excused_apart() {
        let row = |outcome: &str| OutcomeRow {
            event_id: 1,
            user_id: 1,
            outcome: outcome.into(),
            manual_status: None,
        };
        let rows = [
            row("attended"),
            row("attended"),
            row("no_show"),
            row("excused"),
            row("absent"),
        ];
        assert_eq!(
            totals(&rows),
            Totals {
                attended: 2,
                no_shows: 1,
                excused: 1,
                eligible: 3,
                percent: 67
            }
        );
        assert_eq!(totals(&[]).percent, 100);
    }

    const SAMPLE: &str = include_str!("../../../../ci/mrt-attendance-sample.tsv");

    #[test]
    fn reads_the_owners_export() {
        let g = parse_mrt(SAMPLE).unwrap();
        assert_eq!(g.columns.len(), 1);
        let c = &g.columns[0];
        assert_eq!(c.boss, "High Warlord Naj'entus");
        assert_eq!(c.difficulty, Some(4));
        assert_eq!(c.encounter.map(|e| e.0), Some("bt"));
        assert_eq!(c.local.to_string(), "2026-11-15 20:44:00");
        assert_eq!(g.players.len(), 22);
        assert!(g.players.iter().all(|p| p.present == vec![Some(true)]));
        assert!(g
            .players
            .iter()
            .any(|p| p.name == "Shambulàncè" && p.key == "shambulance"));
        assert!(g.players.iter().any(|p| p.name == "Danité"));
        assert!(g.warnings.is_empty());
    }

    #[test]
    fn several_bosses_and_nights_with_gaps() {
        let text = "Black Temple, week 3\r\n\
            \t15/11/2026 20:44:00 - High Warlord Naj'entus4\t15/11/2026 21:10:12 - Supremus4\t\r\n\
            Avizandum\tx\tx\r\n\
            Kitstab\t\tx\r\n\
            Danité-Arugal\tx\t\r\n\
            \n\
            \t16/11/2026 20:05:00 - Shade of Akama4\n\
            Avizandum\tx\n\
            Newbie\tx\n";
        let g = parse_mrt(text).unwrap();
        assert_eq!(g.ignored, vec!["Black Temple, week 3".to_string()]);
        assert_eq!(g.columns.len(), 3);
        assert_eq!(g.columns[1].boss, "Supremus");
        assert_eq!(g.columns[2].local.date().to_string(), "2026-11-16");
        let p = |n: &str| {
            g.players
                .iter()
                .find(|p| p.key == n)
                .unwrap()
                .present
                .clone()
        };
        assert_eq!(p("avizandum"), vec![Some(true), Some(true), Some(true)]);
        assert_eq!(p("kitstab"), vec![Some(false), Some(true), None]);
        assert_eq!(p("danite"), vec![Some(true), Some(false), None]);
        assert_eq!(p("newbie"), vec![None, None, Some(true)]);
    }

    #[test]
    fn spaces_instead_of_tabs() {
        let text = "      15/11/2026 20:44:00 - High Warlord Naj'entus4    15/11/2026 21:10:12 - Supremus4\n\
            Avizandum    x    x\n\
            Kitstab x x\n\
            Baelgunna    x\n";
        let g = parse_mrt(text).unwrap();
        assert_eq!(g.columns.len(), 2);
        assert_eq!(g.columns[0].boss, "High Warlord Naj'entus");
        let p = |n: &str| {
            g.players
                .iter()
                .find(|p| p.key == n)
                .unwrap()
                .present
                .clone()
        };
        assert_eq!(p("avizandum"), vec![Some(true), Some(true)]);
        assert_eq!(p("kitstab"), vec![Some(true), Some(true)]);
        assert_eq!(p("baelgunna"), vec![Some(true), Some(false)]);
        assert_eq!(g.warnings.len(), 1);
    }

    #[test]
    fn difficulty_digits() {
        // No digit at all.
        let (b, d, e) = split_boss("Supremus");
        assert_eq!((b.as_str(), d, e.is_some()), ("Supremus", None, true));
        // A boss that isn't listed keeps its digits…
        let (b, d, e) = split_boss("Kologarn 3000");
        assert_eq!((b.as_str(), d, e.is_none()), ("Kologarn 3000", None, true));
        let (b, d, _) = split_boss("Trial Dummy9");
        assert_eq!((b.as_str(), d), ("Trial Dummy9", None));
        // … unless another column in the paste confirmed that number.
        let g = parse_mrt("\t15/11/2026 20:00:00 - Supremus4\t15/11/2026 21:00:00 - Mystery Boss4\t15/11/2026 22:00:00 - Gate 13\nA\tx\tx\tx\n").unwrap();
        assert_eq!(g.columns[1].boss, "Mystery Boss");
        assert_eq!(g.columns[1].difficulty, Some(4));
        assert_eq!(g.columns[2].boss, "Gate 13");
        assert_eq!(g.columns[2].difficulty, None);
        // 20-player classic raids: 148.
        let (b, d, _) = split_boss("Hakkar148");
        assert_eq!((b.as_str(), d), ("Hakkar", Some(148)));
        // Aliases count (the Reliquary's essences).
        assert!(find_encounter("Essence of Anger").is_some());
    }

    #[test]
    fn not_an_export() {
        assert!(parse_mrt("").is_err());
        assert!(parse_mrt("hello\nworld").is_err());
        assert!(parse_mrt("\t15/11/2026 20:44:00 - Supremus4\n").is_err());
    }

    #[test]
    fn marks() {
        assert!(is_mark("x") && is_mark("X") && is_mark("✓") && is_mark("1"));
        assert!(!is_mark("") && !is_mark(" ") && !is_mark("0") && !is_mark("-"));
    }
}
