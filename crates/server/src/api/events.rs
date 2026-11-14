//! Raid planner: events people sign up for (tank / healer / DPS, class and
//! spec), shown as a live card in chat, with a reminder before they start.
//!
//! Extras: repeating raids (the server makes each raid a few weeks ahead so
//! it has its own sign-ups), a chosen time for the sign-up card to go up
//! (straight away, N days before, or on a set weekday and time; sign-ups
//! open then), sign-up deadlines, an automatic bench when a role is full (first in, first
//! promoted), and attendance (see `attendance.rs`).

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, CreateEventRequest, EventSignup, Message, PostRule, RaidEvent, RaidExport,
    RaidExportEntry, RepeatInfo, SignupCharacter, SignupRequest, UpdateEventRequest,
};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};
use sqlx::types::Json as DbJson;
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};

use super::messages::{fetch_message, post_bot_message};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx, ChannelAccess};
use crate::state::{AppState, Audience};

const STATUSES: &[&str] = &["yes", "late", "tentative", "bench", "no"];
const ROLES: &[&str] = &["tank", "healer", "dps", ""];
/// How long before the start the reminder goes out.
const REMIND_MINUTES: i64 = 15;
/// Repeating raids are made this many days ahead.
const AHEAD_DAYS: i64 = 28;
/// "N days before" for the sign-up card goes up to two weeks.
const MAX_POST_DAYS: i32 = 14;
/// Longest sign-up deadline: a week before the start.
const MAX_DEADLINE_MINUTES: i32 = 7 * 24 * 60;
/// "Remove the chat card after the raid": how long after the end.
const CARD_CLEANUP_HOURS: i32 = 3;

#[derive(sqlx::FromRow)]
pub(crate) struct EventRow {
    pub id: i64,
    pub guild_id: i64,
    pub channel_id: i64,
    pub message_id: Option<i64>,
    pub creator_id: i64,
    pub title: String,
    description: String,
    pub starts_at: DateTime<Utc>,
    duration_minutes: i32,
    max_tanks: i32,
    max_healers: i32,
    max_dps: i32,
    series_id: Option<i64>,
    signup_deadline_minutes: Option<i32>,
    attendance_taken: bool,
    post_rule: Option<DbJson<PostRule>>,
    post_at: Option<DateTime<Utc>>,
    pub game: String,
    card_cleanup: String,
    card_removed_at: Option<DateTime<Utc>>,
    pub soft_reserves: i32,
    pub reserve_lock: String,
    pub reserve_lock_override: Option<bool>,
    pub loot_instance: String,
}

impl EventRow {
    /// Members can't add or change soft reserves (raid leaders still can):
    /// locked by hand, or automatically at the sign-up deadline (else the
    /// start) or at the start, and always once the raid is over.
    pub(crate) fn reserves_locked(&self) -> bool {
        if let Some(locked) = self.reserve_lock_override {
            return locked;
        }
        if self.finished() {
            return true;
        }
        let now = Utc::now();
        match self.reserve_lock.as_str() {
            "manual" => false,
            "start" => now >= self.starts_at,
            _ => now >= self.closes_at().unwrap_or(self.starts_at),
        }
    }

    pub(crate) fn not_open_yet(&self) -> bool {
        self.not_open()
    }

    /// The card isn't up yet and it isn't time for it: sign-ups aren't open.
    fn not_open(&self) -> bool {
        self.message_id.is_none() && self.post_at.is_some_and(|t| t > Utc::now())
    }

    fn max_for(&self, role: &str) -> i32 {
        match role {
            "tank" => self.max_tanks,
            "healer" => self.max_healers,
            "dps" => self.max_dps,
            _ => i32::MAX,
        }
    }

    fn closes_at(&self) -> Option<DateTime<Utc>> {
        self.signup_deadline_minutes
            .map(|m| self.starts_at - chrono::Duration::minutes(m as i64))
    }

    /// Past the sign-up deadline.
    fn locked(&self) -> bool {
        self.closes_at().is_some_and(|t| Utc::now() >= t)
    }

    fn finished(&self) -> bool {
        self.starts_at + chrono::Duration::minutes(self.duration_minutes as i64) <= Utc::now()
    }
}

#[derive(sqlx::FromRow)]
struct SignupRow {
    event_id: i64,
    user_id: i64,
    display_name: String,
    status: String,
    role: String,
    class: String,
    spec: String,
    note: String,
    signed_at: DateTime<Utc>,
    bench_wanted: Option<String>,
    character_id: Option<i64>,
    character_name: String,
    character_last_name: String,
    character_class: String,
    character_game: String,
    character_level: Option<i32>,
    character_item_level: Option<i32>,
    character_guild: Option<String>,
    character_avatar: Option<String>,
}

fn full_name(name: &str, last_name: &str) -> String {
    if last_name.is_empty() {
        name.to_string()
    } else {
        format!("{name} {last_name}")
    }
}

#[derive(sqlx::FromRow)]
struct SeriesRow {
    id: i64,
    guild_id: i64,
    channel_id: i64,
    creator_id: i64,
    title: String,
    description: String,
    duration_minutes: i32,
    max_tanks: i32,
    max_healers: i32,
    max_dps: i32,
    signup_deadline_minutes: Option<i32>,
    time_zone: String,
    start_date: NaiveDate,
    local_time: NaiveTime,
    weekdays: i32,
    every_weeks: i32,
    until_date: Option<NaiveDate>,
    max_count: Option<i32>,
    made_count: i32,
    generated_through: Option<NaiveDate>,
    post_rule: Option<DbJson<PostRule>>,
    game: String,
    card_cleanup: String,
}

pub(crate) const EVENT_COLUMNS: &str = "id, guild_id, channel_id, message_id, creator_id, title,
    description, starts_at, duration_minutes, max_tanks, max_healers, max_dps,
    series_id, signup_deadline_minutes, attendance_taken, post_rule, post_at, game, card_cleanup,
    card_removed_at, soft_reserves, reserve_lock, reserve_lock_override, loot_instance";

const SERIES_COLUMNS: &str = "id, guild_id, channel_id, creator_id, title, description,
    duration_minutes, max_tanks, max_healers, max_dps, signup_deadline_minutes, time_zone,
    start_date, local_time, weekdays, every_weeks, until_date, max_count, made_count,
    generated_through, post_rule, game, card_cleanup";

/// The response to anything that changes a raid: its chat message (with
/// the card) when it has one, otherwise just `{"event": …}`.
#[derive(Serialize)]
#[serde(untagged)]
pub enum EventReply {
    Card(Box<Message>),
    Bare { event: Box<RaidEvent> },
}

async fn signups_for(db: &PgPool, event_ids: &[i64]) -> AppResult<HashMap<i64, Vec<EventSignup>>> {
    let rows = sqlx::query_as::<_, SignupRow>(
        "SELECT s.event_id, s.user_id,
                COALESCE(gm.nickname, u.display_name) AS display_name,
                s.status, s.role, s.class, s.spec, s.note, s.signed_at, s.bench_wanted,
                s.character_id, s.character_name, s.character_last_name, s.character_class,
                s.character_game,
                wc.level AS character_level, wc.item_level AS character_item_level,
                wc.guild_name AS character_guild,
                CASE WHEN wc.avatar_key <> '' THEN '/api/wow/avatars/' || wc.avatar_key || '?v='
                          || COALESCE(floor(extract(epoch FROM wc.last_synced_at))::BIGINT, 0)
                     ELSE '' END AS character_avatar
         FROM event_signups s
         JOIN events e ON e.id = s.event_id
         JOIN users u ON u.id = s.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = s.user_id
         LEFT JOIN wow_characters wc ON wc.id = s.character_id
         WHERE s.event_id = ANY($1)
         ORDER BY s.signed_at, s.user_id",
    )
    .bind(event_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<EventSignup>> = HashMap::new();
    for r in rows {
        let character = (!r.character_name.is_empty()).then(|| SignupCharacter {
            id: r.character_id,
            full_name: full_name(&r.character_name, &r.character_last_name),
            name: r.character_name,
            last_name: r.character_last_name,
            class: r.character_class,
            game: r.character_game,
            level: r.character_level,
            item_level: r.character_item_level,
            guild_name: r.character_guild.unwrap_or_default(),
            avatar: r.character_avatar.unwrap_or_default(),
        });
        map.entry(r.event_id).or_default().push(EventSignup {
            character,
            user_id: r.user_id,
            display_name: r.display_name,
            status: r.status,
            role: r.role,
            class: r.class,
            spec: r.spec,
            note: r.note,
            signed_at: r.signed_at,
            auto_bench: r.bench_wanted.is_some(),
        });
    }
    Ok(map)
}

fn mask_to_days(mask: i32) -> Vec<u8> {
    (0u8..7).filter(|d| mask & (1 << d) != 0).collect()
}

async fn repeat_for(db: &PgPool, series_ids: &[i64]) -> AppResult<HashMap<i64, RepeatInfo>> {
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!("SELECT {SERIES_COLUMNS} FROM event_series WHERE id = ANY($1)");
    let rows = sqlx::query_as::<_, SeriesRow>(&sql)
        .bind(series_ids)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|s| {
            (
                s.id,
                RepeatInfo {
                    weekdays: mask_to_days(s.weekdays),
                    every_weeks: s.every_weeks,
                    until: s.until_date,
                    count: s.max_count,
                    time_zone: s.time_zone,
                },
            )
        })
        .collect())
}

/// Turns rows into full events (sign-ups and repeat details included).
pub(crate) async fn build(db: &PgPool, rows: Vec<EventRow>) -> AppResult<Vec<RaidEvent>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut signups = signups_for(db, &ids).await?;
    let mut series_ids: Vec<i64> = rows.iter().filter_map(|r| r.series_id).collect();
    series_ids.sort_unstable();
    series_ids.dedup();
    let repeats = repeat_for(db, &series_ids).await?;
    let mut reserves = super::loot::reserves_for(db, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let closes = r.closes_at();
            let reserves_locked = r.reserves_locked();
            let opens = if r.message_id.is_none() && r.card_removed_at.is_none() {
                r.post_at
            } else {
                None
            };
            RaidEvent {
                post_rule: r.post_rule.map(|j| j.0),
                signups_open_at: opens,
                game: r.game,
                card_cleanup: r.card_cleanup,
                card_removed: r.card_removed_at.is_some(),
                signups: signups.remove(&r.id).unwrap_or_default(),
                repeat: r.series_id.and_then(|s| repeats.get(&s).cloned()),
                id: r.id,
                guild_id: r.guild_id,
                channel_id: r.channel_id,
                message_id: r.message_id,
                creator_id: r.creator_id,
                title: r.title,
                description: r.description,
                starts_at: r.starts_at,
                duration_minutes: r.duration_minutes,
                max_tanks: r.max_tanks,
                max_healers: r.max_healers,
                max_dps: r.max_dps,
                series_id: r.series_id,
                signup_deadline_minutes: r.signup_deadline_minutes,
                signups_close_at: closes,
                attendance_taken: r.attendance_taken,
                soft_reserves: r.soft_reserves,
                reserve_lock: r.reserve_lock,
                reserve_lock_override: r.reserve_lock_override,
                reserves_locked,
                loot_instance_name: outhouse_common::raidplan::instance_name(&r.loot_instance),
                loot_instance: r.loot_instance,
                reserves: reserves.remove(&r.id).unwrap_or_default(),
            }
        })
        .collect())
}

/// Events attached to these messages, keyed by message ID (used when
/// loading messages).
pub async fn events_for_messages(
    db: &PgPool,
    message_ids: &[i64],
) -> AppResult<HashMap<i64, RaidEvent>> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM events WHERE message_id = ANY($1)");
    let rows = sqlx::query_as::<_, EventRow>(&sql)
        .bind(message_ids)
        .fetch_all(db)
        .await?;
    Ok(build(db, rows)
        .await?
        .into_iter()
        .filter_map(|e| e.message_id.map(|m| (m, e)))
        .collect())
}

pub(crate) async fn load_row(db: &PgPool, event_id: i64) -> AppResult<EventRow> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = $1");
    sqlx::query_as::<_, EventRow>(&sql)
        .bind(event_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

/// The event plus the caller's access to its channel.
pub(crate) async fn load(
    st: &AppState,
    event_id: i64,
    user_id: i64,
) -> AppResult<(EventRow, ChannelAccess)> {
    let row = load_row(&st.db, event_id).await?;
    let access = channel_access(&st.db, row.channel_id, user_id).await?;
    Ok((row, access))
}

/// Raid leaders: whoever planned it, anyone with Manage Events, and anyone
/// with one of the outhouse's raid manager roles (Outhouse settings → Raids).
pub(crate) async fn can_manage(
    db: &PgPool,
    row: &EventRow,
    access: &ChannelAccess,
) -> AppResult<bool> {
    if row.creator_id == access.ctx.user_id || access.perms & p::MANAGE_EVENTS != 0 {
        return Ok(true);
    }
    super::raid_settings::has_manager_role(db, &access.ctx).await
}

/// Tells everyone about the raid's new state (its card, calendars) and
/// returns it.
pub(crate) async fn broadcast(st: &AppState, event_id: i64) -> AppResult<EventReply> {
    let row = load_row(&st.db, event_id).await?;
    let audience = Audience::Channel {
        guild_id: row.guild_id,
        channel_id: row.channel_id,
    };
    let event = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    st.dispatch(audience.clone(), ServerEvent::EventUpdate(event.clone()));
    match event.message_id {
        Some(message_id) => {
            let message = fetch_message(&st.db, message_id).await?;
            st.dispatch(audience, ServerEvent::MessageUpdate(message.clone()));
            Ok(EventReply::Card(Box::new(message)))
        }
        None => Ok(EventReply::Bare {
            event: Box::new(event),
        }),
    }
}

fn clean_title(title: &str) -> AppResult<String> {
    let t: String = title.trim().chars().take(100).collect();
    if t.is_empty() {
        return Err(AppError::BadRequest("Give the raid a name.".into()));
    }
    Ok(t)
}

fn clean_text(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

fn check_slots(n: Option<i32>) -> AppResult<Option<i32>> {
    match n {
        Some(v) if !(0..=40).contains(&v) => Err(AppError::BadRequest(
            "Role slots must be between 0 and 40.".into(),
        )),
        other => Ok(other),
    }
}

fn check_duration(n: Option<i32>) -> AppResult<Option<i32>> {
    match n {
        Some(v) if !(15..=720).contains(&v) => Err(AppError::BadRequest(
            "Length must be between 15 minutes and 12 hours.".into(),
        )),
        other => Ok(other),
    }
}

/// `None`: not given. `Some(None)`: no deadline. `Some(Some(m))`: m minutes.
fn check_deadline(n: Option<i32>) -> AppResult<Option<Option<i32>>> {
    match n {
        None => Ok(None),
        Some(0) => Ok(Some(None)),
        Some(v) if (1..=MAX_DEADLINE_MINUTES).contains(&v) => Ok(Some(Some(v))),
        Some(_) => Err(AppError::BadRequest(
            "Sign-ups can close up to a week before the raid.".into(),
        )),
    }
}

/// "" (any game) or a known game id.
fn check_game(raw: Option<&str>) -> AppResult<Option<String>> {
    match raw.map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(String::new())),
        Some(g) => outhouse_common::wow::game_id(g)
            .map(|id| Some(id.to_string()))
            .ok_or_else(|| AppError::BadRequest("Unknown game.".into())),
    }
}

/// "off", "after" or "next" ("next" only for repeating raids).
fn check_card_cleanup(raw: Option<&str>, repeating: bool) -> AppResult<Option<String>> {
    match raw.map(str::trim) {
        None => Ok(None),
        Some(v @ ("off" | "after")) => Ok(Some(v.to_string())),
        Some("next") if repeating => Ok(Some("next".into())),
        Some("next") => Err(AppError::BadRequest(
            "Only repeating raids can wait for the next raid's sign-ups.".into(),
        )),
        Some(_) => Err(AppError::BadRequest(
            "Keep the chat card, remove it 3 hours after the raid, or when the next raid's sign-ups go up.".into(),
        )),
    }
}

/// A known IANA time zone (also used for an outhouse's realm time).
pub(crate) async fn check_time_zone(db: &PgPool, raw: &str) -> AppResult<String> {
    check_tz(db, raw).await
}

async fn check_tz(db: &PgPool, raw: &str) -> AppResult<String> {
    let tz = raw.trim().to_string();
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name = $1)")
            .bind(&tz)
            .fetch_one(db)
            .await?;
    if tz.is_empty() || tz.len() > 64 || !known {
        return Err(AppError::BadRequest("Unknown time zone.".into()));
    }
    Ok(tz)
}

/// Tidies a sign-up posting rule. `None` (or "now"): straight away.
/// `default_tz` is used when the rule doesn't say (a series' own zone).
async fn check_post_rule(
    db: &PgPool,
    rule: Option<&PostRule>,
    default_tz: &str,
) -> AppResult<Option<PostRule>> {
    let Some(rule) = rule else {
        return Ok(None);
    };
    let tz = match rule.time_zone.as_deref().map(str::trim) {
        Some(tz) if !tz.is_empty() => check_tz(db, tz).await?,
        _ => default_tz.to_string(),
    };
    match rule.mode.as_str() {
        "" | "now" => Ok(None),
        "days" => {
            let days = rule.days.unwrap_or(7);
            if !(1..=MAX_POST_DAYS).contains(&days) {
                return Err(AppError::BadRequest(format!(
                    "Sign-ups can go up 1 to {MAX_POST_DAYS} days before."
                )));
            }
            Ok(Some(PostRule {
                mode: "days".into(),
                days: Some(days),
                time_zone: Some(tz),
                ..Default::default()
            }))
        }
        "weekday" => {
            let weekday = rule.weekday.unwrap_or(0);
            if weekday > 6 {
                return Err(AppError::BadRequest("Unknown day of the week.".into()));
            }
            let time = rule.time.as_deref().unwrap_or("09:00").trim();
            let parsed = NaiveTime::parse_from_str(time, "%H:%M")
                .map_err(|_| AppError::BadRequest("Give the posting time as HH:MM.".into()))?;
            Ok(Some(PostRule {
                mode: "weekday".into(),
                weekday: Some(weekday),
                time: Some(parsed.format("%H:%M").to_string()),
                time_zone: Some(tz),
                ..Default::default()
            }))
        }
        _ => Err(AppError::BadRequest(
            "Post sign-ups right away, some days before, or on a set day.".into(),
        )),
    }
}

/// When the sign-up card for a raid starting at `starts_at` goes up
/// (`None`: straight away). Worked out in the rule's time zone, so a change
/// to or from daylight saving in between doesn't move it.
async fn post_at_for<'c, E: PgExecutor<'c>>(
    ex: E,
    rule: Option<&PostRule>,
    starts_at: DateTime<Utc>,
) -> AppResult<Option<DateTime<Utc>>> {
    let Some(rule) = rule else {
        return Ok(None);
    };
    let tz = rule.time_zone.clone().unwrap_or_else(|| "UTC".into());
    let at: DateTime<Utc> = match rule.mode.as_str() {
        "days" => {
            sqlx::query_scalar(
                "SELECT (($1::timestamptz AT TIME ZONE $2) - make_interval(days => $3)) AT TIME ZONE $2",
            )
            .bind(starts_at)
            .bind(&tz)
            .bind(rule.days.unwrap_or(7))
            .fetch_one(ex)
            .await?
        }
        "weekday" => {
            let time = rule
                .time
                .as_deref()
                .and_then(|t| NaiveTime::parse_from_str(t, "%H:%M").ok())
                .unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("9am is a time"));
            // The last such weekday before the raid's day: 1 to 7 days back.
            sqlx::query_scalar(
                "WITH l AS (SELECT ($1::timestamptz AT TIME ZONE $2) AS lt),
                      d AS (SELECT lt, ((EXTRACT(ISODOW FROM lt)::int - 1 - $3) % 7 + 7) % 7 AS back
                            FROM l)
                 SELECT ((lt::date - (CASE WHEN back = 0 THEN 7 ELSE back END)) + $4::time)
                        AT TIME ZONE $2
                 FROM d",
            )
            .bind(starts_at)
            .bind(&tz)
            .bind(rule.weekday.unwrap_or(0) as i32)
            .bind(time)
            .fetch_one(ex)
            .await?
        }
        _ => return Ok(None),
    };
    Ok(Some(at))
}

/// Works out the posting time again for raids whose card isn't up yet.
async fn refresh_post_times(st: &AppState, ids: &[i64]) -> AppResult<()> {
    for &id in ids {
        let row = load_row(&st.db, id).await?;
        if row.message_id.is_some() {
            continue;
        }
        let at = post_at_for(&st.db, row.post_rule.as_ref().map(|j| &j.0), row.starts_at).await?;
        sqlx::query("UPDATE events SET post_at = $2 WHERE id = $1 AND message_id IS NULL")
            .bind(id)
            .bind(at)
            .execute(&st.db)
            .await?;
    }
    Ok(())
}

// ---- Repeating raids ---------------------------------------------------

struct Pattern {
    mask: i32,
    every_weeks: i32,
    until: Option<NaiveDate>,
    count: Option<i32>,
    time_zone: String,
}

async fn check_pattern(db: &PgPool, r: &RepeatInfo) -> AppResult<Pattern> {
    let mut mask = 0;
    for &d in &r.weekdays {
        if d > 6 {
            return Err(AppError::BadRequest("Unknown day of the week.".into()));
        }
        mask |= 1 << d;
    }
    if mask == 0 {
        return Err(AppError::BadRequest(
            "Pick at least one day for it to repeat on.".into(),
        ));
    }
    if !(1..=8).contains(&r.every_weeks) {
        return Err(AppError::BadRequest(
            "It can repeat every 1 to 8 weeks.".into(),
        ));
    }
    if r.count.is_some_and(|c| !(1..=104).contains(&c)) {
        return Err(AppError::BadRequest(
            "A series can have 1 to 104 raids.".into(),
        ));
    }
    let tz = check_tz(db, &r.time_zone).await?;
    Ok(Pattern {
        mask,
        every_weeks: r.every_weeks,
        until: r.until,
        count: r.count,
        time_zone: tz,
    })
}

/// A moment as the wall-clock time in a time zone.
async fn local_time_in(db: &PgPool, at: DateTime<Utc>, tz: &str) -> AppResult<NaiveDateTime> {
    Ok(
        sqlx::query_scalar("SELECT ($1::timestamptz AT TIME ZONE $2)")
            .bind(at)
            .bind(tz)
            .fetch_one(db)
            .await?,
    )
}

/// Makes the series' raids up to [`AHEAD_DAYS`] ahead (at least one if
/// `at_least_one`). Returns the new raids' IDs.
async fn materialise(st: &AppState, series_id: i64, at_least_one: bool) -> AppResult<Vec<i64>> {
    let mut tx = st.db.begin().await?;
    let (new_ids, _) =
        materialise_tx(st, &mut tx, series_id, at_least_one, &HashMap::new()).await?;
    tx.commit().await?;
    dispatch_events(st, &new_ids).await;
    Ok(new_ids)
}

/// Sends the current state of these raids to whoever can see them.
async fn dispatch_events(st: &AppState, ids: &[i64]) {
    for &id in ids {
        if let Ok(row) = load_row(&st.db, id).await {
            if let Ok(mut ev) = build(&st.db, vec![row]).await {
                if let Some(ev) = ev.pop() {
                    st.dispatch(
                        Audience::Channel {
                            guild_id: ev.guild_id,
                            channel_id: ev.channel_id,
                        },
                        ServerEvent::EventUpdate(ev),
                    );
                }
            }
        }
    }
}

/// The days a repeat pattern lands on: from `start` (weeks are counted
/// from the week `start` is in), up to and including `to`, at most `limit`
/// of them, and never past `until` or more than `count` in all.
fn pattern_days(
    start: NaiveDate,
    mask: i32,
    every_weeks: i32,
    until: Option<NaiveDate>,
    count: Option<i32>,
    to: NaiveDate,
    limit: usize,
) -> Vec<NaiveDate> {
    let week0 = start - chrono::Duration::days(start.weekday().num_days_from_monday() as i64);
    let every = every_weeks.max(1) as i64;
    let mut out = Vec::new();
    let mut day = start;
    while day <= to && out.len() < limit {
        if until.is_some_and(|u| day > u) || count.is_some_and(|c| out.len() as i64 >= c as i64) {
            break;
        }
        let week = (day - week0).num_days() / 7;
        if week % every == 0 && mask & (1 << day.weekday().num_days_from_monday()) != 0 {
            out.push(day);
        }
        day = day + chrono::Duration::days(1);
    }
    out
}

/// [`materialise`] inside a transaction. Raids in `adopt` (by day, in the
/// series' time zone) already exist: when the pattern lands on their day
/// they join the series and take its details instead of a new raid being
/// made (they keep their sign-ups). Returns (new raids, adopted raids).
async fn materialise_tx(
    st: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    series_id: i64,
    at_least_one: bool,
    adopt: &HashMap<NaiveDate, i64>,
) -> AppResult<(Vec<i64>, Vec<i64>)> {
    let sql =
        format!("SELECT {SERIES_COLUMNS} FROM event_series WHERE id = $1 AND NOT ended FOR UPDATE");
    let Some(s) = sqlx::query_as::<_, SeriesRow>(&sql)
        .bind(series_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let now = Utc::now();
    let horizon = (now + chrono::Duration::days(AHEAD_DAYS + 1)).date_naive();
    let week0 =
        s.start_date - chrono::Duration::days(s.start_date.weekday().num_days_from_monday() as i64);
    let mut day = match s.generated_through {
        Some(g) => g + chrono::Duration::days(1),
        None => s.start_date,
    };
    let mut through = s.generated_through;
    let mut made = s.made_count;
    let mut ended = false;
    let mut new_ids = Vec::new();
    let mut adopted = Vec::new();
    // At most every 8 weeks: a match always turns up within 56 days.
    for _ in 0..800 {
        if day > horizon && !(at_least_one && new_ids.is_empty() && adopted.is_empty()) {
            break;
        }
        if s.until_date.is_some_and(|u| day > u) || s.max_count.is_some_and(|c| made >= c) {
            ended = true;
            break;
        }
        let week = (day - week0).num_days() / 7;
        let weekday = day.weekday().num_days_from_monday();
        if week % s.every_weeks as i64 == 0 && s.weekdays & (1 << weekday) != 0 {
            made += 1;
            let starts_at: DateTime<Utc> =
                sqlx::query_scalar("SELECT ($1::date + $2::time) AT TIME ZONE $3")
                    .bind(day)
                    .bind(s.local_time)
                    .bind(&s.time_zone)
                    .fetch_one(&mut **tx)
                    .await?;
            if let Some(&id) = adopt.get(&day) {
                // An existing raid on this day: it stays (with its sign-ups).
                sqlx::query(
                    "UPDATE events SET series_id = $2, starts_at = $3, title = $4,
                            description = $5, duration_minutes = $6, max_tanks = $7,
                            max_healers = $8, max_dps = $9, signup_deadline_minutes = $10,
                            post_rule = $11, game = $12, card_cleanup = $13,
                            reminded = reminded AND starts_at = $3
                     WHERE id = $1",
                )
                .bind(id)
                .bind(s.id)
                .bind(starts_at)
                .bind(&s.title)
                .bind(&s.description)
                .bind(s.duration_minutes)
                .bind(s.max_tanks)
                .bind(s.max_healers)
                .bind(s.max_dps)
                .bind(s.signup_deadline_minutes)
                .bind(&s.post_rule)
                .bind(&s.game)
                .bind(&s.card_cleanup)
                .execute(&mut **tx)
                .await?;
                adopted.push(id);
            } else if starts_at > now - chrono::Duration::minutes(30) {
                // Skip ones already over (e.g. a series that started today, earlier on).
                let id = st.ids.next_id();
                let rule = s.post_rule.as_ref().map(|j| &j.0);
                let post_at = post_at_for(&mut **tx, rule, starts_at).await?;
                sqlx::query(
                    "INSERT INTO events (id, guild_id, channel_id, message_id, creator_id, title,
                         description, starts_at, duration_minutes, max_tanks, max_healers,
                         max_dps, series_id, signup_deadline_minutes, post_rule, post_at, game,
                         card_cleanup)
                     VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                             $15, $16, $17)",
                )
                .bind(id)
                .bind(s.guild_id)
                .bind(s.channel_id)
                .bind(s.creator_id)
                .bind(&s.title)
                .bind(&s.description)
                .bind(starts_at)
                .bind(s.duration_minutes)
                .bind(s.max_tanks)
                .bind(s.max_healers)
                .bind(s.max_dps)
                .bind(s.id)
                .bind(s.signup_deadline_minutes)
                .bind(&s.post_rule)
                .bind(post_at)
                .bind(&s.game)
                .bind(&s.card_cleanup)
                .execute(&mut **tx)
                .await?;
                new_ids.push(id);
            }
        }
        through = Some(day);
        day = day + chrono::Duration::days(1);
    }
    sqlx::query(
        "UPDATE event_series SET made_count = $2, generated_through = $3, ended = $4 WHERE id = $1",
    )
    .bind(s.id)
    .bind(made)
    .bind(through)
    .bind(ended)
    .execute(&mut **tx)
    .await?;
    Ok((new_ids, adopted))
}

/// Posts the chat card for raids whose posting time has come and that
/// don't have one yet (only this series' raids, or these raids, if given).
async fn post_due_cards(
    st: &AppState,
    series_id: Option<i64>,
    ids: Option<&[i64]>,
) -> AppResult<()> {
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM events
         WHERE message_id IS NULL
           AND (post_at IS NULL OR post_at <= now())
           AND starts_at + make_interval(mins => duration_minutes) > now()
           AND ($1::BIGINT IS NULL OR series_id = $1)
           AND ($2::BIGINT[] IS NULL OR id = ANY($2))
         ORDER BY starts_at, id
         LIMIT 100"
    );
    let rows = sqlx::query_as::<_, EventRow>(&sql)
        .bind(series_id)
        .bind(ids)
        .fetch_all(&st.db)
        .await?;
    for row in rows {
        let message_id = st.ids.next_id();
        let mut tx = st.db.begin().await?;
        sqlx::query(
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES ($1, $2, $3, '')",
        )
        .bind(message_id)
        .bind(row.channel_id)
        .bind(row.creator_id)
        .execute(&mut *tx)
        .await?;
        let claimed = sqlx::query(
            "UPDATE events SET message_id = $2, card_posted_at = now(), card_removed_at = NULL
                 WHERE id = $1 AND message_id IS NULL",
        )
        .bind(row.id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if claimed == 0 {
            // Someone else posted it first.
            tx.rollback().await?;
            continue;
        }
        sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
            .bind(row.channel_id)
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let message = fetch_message(&st.db, message_id).await?;
        let audience = Audience::Channel {
            guild_id: row.guild_id,
            channel_id: row.channel_id,
        };
        if let Some(ev) = &message.event {
            // Calendars drop the "sign-ups open …" label.
            st.dispatch(audience.clone(), ServerEvent::EventUpdate(ev.clone()));
        }
        st.dispatch(audience, ServerEvent::MessageCreate(message));
    }
    // A new card may be what an older raid's card was waiting for.
    if let Err(e) = remove_old_cards(st).await {
        tracing::warn!(error = %e, "removing old raid cards failed");
    }
    Ok(())
}

/// Removes the chat cards of finished raids that asked for it ("Remove the
/// chat card after the raid"). Only the message goes: the raid keeps its
/// sign-ups, attendance and calendar entry, and a thread started on the
/// card stays in the channel's thread list. Pinned cards are left alone
/// (someone wanted them kept). Safe to run any number of times.
async fn remove_old_cards(st: &AppState) -> AppResult<()> {
    let due: Vec<(i64, i64, i64, i64)> = sqlx::query_as(&format!(
        "WITH c AS (
             SELECT e.id, e.guild_id, e.channel_id, e.message_id, e.series_id, e.starts_at,
                    e.card_cleanup,
                    e.starts_at + make_interval(mins => e.duration_minutes) AS ends_at
             FROM events e
             JOIN messages m ON m.id = e.message_id
             WHERE e.card_cleanup <> 'off'
               AND m.pinned_at IS NULL
               AND e.starts_at + make_interval(mins => e.duration_minutes) <= now())
         SELECT id, guild_id, channel_id, message_id FROM c
         WHERE (card_cleanup = 'after'
                AND now() >= ends_at + make_interval(hours => {CARD_CLEANUP_HOURS}))
            OR (card_cleanup = 'next' AND series_id IS NOT NULL AND EXISTS (
                    SELECT 1 FROM events n
                    WHERE (n.series_id = c.series_id
                           OR n.series_id IN (SELECT id FROM event_series
                                              WHERE split_from = c.series_id))
                      AND n.starts_at > c.starts_at
                      AND n.card_posted_at IS NOT NULL
                      AND (n.card_posted_at >= c.ends_at
                           OR now() >= c.ends_at + make_interval(hours => {CARD_CLEANUP_HOURS}))))
         ORDER BY starts_at
         LIMIT 100"
    ))
    .fetch_all(&st.db)
    .await?;
    for (event_id, guild_id, channel_id, message_id) in due {
        let mut tx = st.db.begin().await?;
        // Unhook the card first: deleting a raid's message would delete the raid.
        let claimed = sqlx::query(
            "UPDATE events SET message_id = NULL, card_removed_at = now()
             WHERE id = $1 AND message_id = $2",
        )
        .bind(event_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if claimed == 0 {
            tx.rollback().await?;
            continue;
        }
        sqlx::query("DELETE FROM messages WHERE id = $1")
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let audience = Audience::Channel {
            guild_id,
            channel_id,
        };
        st.dispatch(
            audience.clone(),
            ServerEvent::MessageDelete {
                id: message_id,
                channel_id,
            },
        );
        if let Ok(row) = load_row(&st.db, event_id).await {
            if let Ok(Some(ev)) = build(&st.db, vec![row]).await.map(|mut v| v.pop()) {
                st.dispatch(audience, ServerEvent::EventUpdate(ev));
            }
        }
    }
    Ok(())
}

/// What "Remove the chat card after the raid" starts on in this outhouse
/// (repeating raids, one-off raids).
pub(crate) async fn card_cleanup_defaults(
    db: &PgPool,
    guild_id: i64,
) -> AppResult<(String, String)> {
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT card_cleanup_series, card_cleanup_single FROM raid_settings WHERE guild_id = $1",
    )
    .bind(guild_id)
    .fetch_optional(db)
    .await?;
    Ok(found.unwrap_or_else(|| ("next".into(), "off".into())))
}

/// Every minute: make the next raids of every series, and post due cards.
async fn run_series(st: &AppState) -> AppResult<()> {
    let ids: Vec<i64> = sqlx::query_scalar(&format!(
        "SELECT id FROM event_series
         WHERE NOT ended
           AND (generated_through IS NULL
                OR generated_through < (now() + make_interval(days => {AHEAD_DAYS}))::date)"
    ))
    .fetch_all(&st.db)
    .await?;
    for id in ids {
        if let Err(e) = materialise(st, id, false).await {
            tracing::warn!(error = %e, series = id, "making repeating raids failed");
        }
    }
    post_due_cards(st, None, None).await
}

// ---- Soft-reserve settings ---------------------------------------------

/// Most soft reserves per person.
const MAX_SOFT_RESERVES: i32 = 2;

/// Soft-reserve settings from Plan/Edit raid (left out: unchanged).
struct LootChange {
    soft_reserves: Option<i32>,
    reserve_lock: Option<String>,
    loot_instance: Option<String>,
}

impl LootChange {
    fn any(&self) -> bool {
        self.soft_reserves.is_some() || self.reserve_lock.is_some() || self.loot_instance.is_some()
    }
}

fn check_loot(
    soft: Option<i32>,
    lock: Option<&str>,
    instance: Option<&str>,
) -> AppResult<LootChange> {
    if soft.is_some_and(|n| !(0..=MAX_SOFT_RESERVES).contains(&n)) {
        return Err(AppError::BadRequest(
            "Soft reserves are off, 1 or 2 per person.".into(),
        ));
    }
    let reserve_lock =
        match lock.map(str::trim) {
            None => None,
            Some("") => Some("deadline".to_string()),
            Some(v @ ("deadline" | "start" | "manual")) => Some(v.to_string()),
            Some(_) => return Err(AppError::BadRequest(
                "Reserves lock when sign-ups close, when the raid starts, or when you lock them."
                    .into(),
            )),
        };
    let loot_instance = instance.map(|raw| {
        let t = clean_text(raw, 60);
        match outhouse_common::raidplan::instance(&outhouse_common::raidplan::instance_key(&t)) {
            Some(i) => i.0.to_string(),
            None => t,
        }
    });
    Ok(LootChange {
        soft_reserves: soft,
        reserve_lock,
        loot_instance,
    })
}

fn reply_event_id(reply: &EventReply) -> Option<i64> {
    match reply {
        EventReply::Card(m) => m.event.as_ref().map(|e| e.id),
        EventReply::Bare { event } => Some(event.id),
    }
}

/// Saves soft-reserve settings on a raid (and, for "this and future
/// raids", on the rest of its series and the raids it makes later).
async fn apply_loot(st: &AppState, event_id: i64, future: bool, c: &LootChange) -> AppResult<()> {
    let row = match load_row(&st.db, event_id).await {
        Ok(r) => r,
        Err(AppError::NotFound) => return Ok(()),
        Err(e) => return Err(e),
    };
    let soft = c.soft_reserves.unwrap_or(row.soft_reserves);
    let lock = c
        .reserve_lock
        .clone()
        .unwrap_or_else(|| row.reserve_lock.clone());
    let instance = c
        .loot_instance
        .clone()
        .unwrap_or_else(|| row.loot_instance.clone());
    let ids: Vec<i64> = match (future, row.series_id) {
        (true, Some(series_id)) => {
            sqlx::query(
                "UPDATE event_series SET soft_reserves = $2, reserve_lock = $3, loot_instance = $4
                 WHERE id = $1",
            )
            .bind(series_id)
            .bind(soft)
            .bind(&lock)
            .bind(&instance)
            .execute(&st.db)
            .await?;
            sqlx::query_scalar(
                "UPDATE events SET soft_reserves = $3, reserve_lock = $4, loot_instance = $5
                 WHERE series_id = $1 AND starts_at >= $2
                 RETURNING id",
            )
            .bind(series_id)
            .bind(row.starts_at)
            .bind(soft)
            .bind(&lock)
            .bind(&instance)
            .fetch_all(&st.db)
            .await?
        }
        _ => {
            sqlx::query(
                "UPDATE events SET soft_reserves = $2, reserve_lock = $3, loot_instance = $4
                 WHERE id = $1",
            )
            .bind(event_id)
            .bind(soft)
            .bind(&lock)
            .bind(&instance)
            .execute(&st.db)
            .await?;
            vec![event_id]
        }
    };
    // Fewer reserves each: people keep their first ones.
    if soft > 0 {
        sqlx::query(
            "DELETE FROM loot_reserves r
             USING (SELECT event_id, user_id, item_id,
                           row_number() OVER (PARTITION BY event_id, user_id
                                              ORDER BY created_at, item_id) AS n
                    FROM loot_reserves WHERE event_id = ANY($1)) x
             WHERE r.event_id = x.event_id AND r.user_id = x.user_id
               AND r.item_id = x.item_id AND x.n > $2",
        )
        .bind(&ids)
        .bind(soft as i64)
        .execute(&st.db)
        .await?;
    }
    for &id in ids.iter().filter(|&&id| id != event_id) {
        broadcast(st, id).await?;
    }
    Ok(())
}

// ---- Endpoints ---------------------------------------------------------

/// Posts a new raid in a channel (or starts a repeating series).
pub async fn create(
    State(st): State<AppState>,
    user: AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateEventRequest>,
) -> AppResult<Json<EventReply>> {
    let loot = check_loot(
        req.soft_reserves,
        req.reserve_lock.as_deref(),
        req.loot_instance.as_deref(),
    )?;
    let reply = create_core(State(st.clone()), user, Path(channel_id), Json(req)).await?;
    if !loot.any() {
        return Ok(reply);
    }
    let Some(event_id) = reply_event_id(&reply.0) else {
        return Ok(reply);
    };
    apply_loot(&st, event_id, true, &loot).await?;
    Ok(Json(broadcast(&st, event_id).await?))
}

async fn create_core(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateEventRequest>,
) -> AppResult<Json<EventReply>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    if access.channel.guild_id == outhouse_common::models::DM_GUILD {
        return Err(AppError::BadRequest(
            "Raids are planned in an outhouse, not in DMs.".into(),
        ));
    }
    if access.channel.kind == channel_kind::VOICE {
        return Err(AppError::BadRequest(
            "Raid sign-ups go in a text channel.".into(),
        ));
    }
    let game = check_game(req.game.as_deref())?.unwrap_or_default();
    let title = clean_title(&req.title)?;
    let description = clean_text(&req.description, 1000);
    let duration = check_duration(req.duration_minutes)?.unwrap_or(180);
    // Defaults: a 40-player raid.
    let tanks = check_slots(req.max_tanks)?.unwrap_or(5);
    let healers = check_slots(req.max_healers)?.unwrap_or(10);
    let dps = check_slots(req.max_dps)?.unwrap_or(25);
    let deadline = check_deadline(req.signup_deadline_minutes)?.flatten();
    let card_cleanup = match check_card_cleanup(req.card_cleanup.as_deref(), req.repeat.is_some())?
    {
        Some(c) => c,
        None => {
            let (series, single) = card_cleanup_defaults(&st.db, access.channel.guild_id).await?;
            if req.repeat.is_some() {
                series
            } else {
                single
            }
        }
    };
    if req.starts_at < Utc::now() - chrono::Duration::hours(1) {
        return Err(AppError::BadRequest("That time has already passed.".into()));
    }
    if req.starts_at > Utc::now() + chrono::Duration::days(366) {
        return Err(AppError::BadRequest(
            "Raids can be planned up to a year ahead.".into(),
        ));
    }
    let guild_id = access.channel.guild_id;

    if let Some(repeat) = &req.repeat {
        let pat = check_pattern(&st.db, repeat).await?;
        // Without a rule, a series' cards go up a week ahead (as they always did).
        let week_before = PostRule {
            mode: "days".into(),
            days: Some(7),
            ..Default::default()
        };
        let post_rule = check_post_rule(
            &st.db,
            Some(req.post_rule.as_ref().unwrap_or(&week_before)),
            &pat.time_zone,
        )
        .await?;
        let local = local_time_in(&st.db, req.starts_at, &pat.time_zone).await?;
        if pat.until.is_some_and(|u| u < local.date()) {
            return Err(AppError::BadRequest(
                "The end date is before the first raid.".into(),
            ));
        }
        if pat
            .until
            .is_some_and(|u| u > local.date() + chrono::Duration::days(731))
        {
            return Err(AppError::BadRequest(
                "A series can run for up to two years.".into(),
            ));
        }
        let series_id = st.ids.next_id();
        sqlx::query(
            "INSERT INTO event_series (id, guild_id, channel_id, creator_id, title, description,
                 duration_minutes, max_tanks, max_healers, max_dps, signup_deadline_minutes,
                 time_zone, start_date, local_time, weekdays, every_weeks, until_date, max_count,
                 post_rule, game, card_cleanup)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                     $18, $19, $20, $21)",
        )
        .bind(series_id)
        .bind(guild_id)
        .bind(channel_id)
        .bind(user.id)
        .bind(&title)
        .bind(&description)
        .bind(duration)
        .bind(tanks)
        .bind(healers)
        .bind(dps)
        .bind(deadline)
        .bind(&pat.time_zone)
        .bind(local.date())
        .bind(local.time())
        .bind(pat.mask)
        .bind(pat.every_weeks)
        .bind(pat.until)
        .bind(pat.count)
        .bind(post_rule.map(DbJson))
        .bind(&game)
        .bind(&card_cleanup)
        .execute(&st.db)
        .await?;
        let made = materialise(&st, series_id, true).await?;
        let Some(&first) = made.first() else {
            sqlx::query("DELETE FROM event_series WHERE id = $1")
                .bind(series_id)
                .execute(&st.db)
                .await?;
            return Err(AppError::BadRequest(
                "That pattern doesn't have any raids in it.".into(),
            ));
        };
        post_due_cards(&st, Some(series_id), None).await?;
        return Ok(Json(broadcast(&st, first).await?));
    }

    let post_rule = check_post_rule(&st.db, req.post_rule.as_ref(), "UTC").await?;
    let post_at = post_at_for(&st.db, post_rule.as_ref(), req.starts_at).await?;
    let event_id = st.ids.next_id();
    if post_at.is_some_and(|t| t > Utc::now()) {
        // The card goes up later; until then it's only in the calendar.
        sqlx::query(
            "INSERT INTO events (id, guild_id, channel_id, message_id, creator_id, title,
                 description, starts_at, duration_minutes, max_tanks, max_healers, max_dps,
                 signup_deadline_minutes, post_rule, post_at, game, card_cleanup)
             VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(event_id)
        .bind(guild_id)
        .bind(channel_id)
        .bind(user.id)
        .bind(&title)
        .bind(&description)
        .bind(req.starts_at)
        .bind(duration)
        .bind(tanks)
        .bind(healers)
        .bind(dps)
        .bind(deadline)
        .bind(post_rule.map(DbJson))
        .bind(post_at)
        .bind(&game)
        .bind(&card_cleanup)
        .execute(&st.db)
        .await?;
        return Ok(Json(broadcast(&st, event_id).await?));
    }

    let message_id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, channel_id, author_id, content) VALUES ($1, $2, $3, '')",
    )
    .bind(message_id)
    .bind(channel_id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO events (id, guild_id, channel_id, message_id, creator_id, title, description,
                             starts_at, duration_minutes, max_tanks, max_healers, max_dps,
                             signup_deadline_minutes, post_rule, game, card_cleanup,
                             card_posted_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, now())",
    )
    .bind(event_id)
    .bind(guild_id)
    .bind(channel_id)
    .bind(message_id)
    .bind(user.id)
    .bind(&title)
    .bind(&description)
    .bind(req.starts_at)
    .bind(duration)
    .bind(tanks)
    .bind(healers)
    .bind(dps)
    .bind(deadline)
    .bind(post_rule.map(DbJson))
    .bind(&game)
    .bind(&card_cleanup)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let message = fetch_message(&st.db, message_id).await?;
    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id,
        },
        ServerEvent::MessageCreate(message.clone()),
    );
    if let Some(ev) = &message.event {
        st.dispatch(
            Audience::Channel {
                guild_id,
                channel_id,
            },
            ServerEvent::EventUpdate(ev.clone()),
        );
    }
    Ok(Json(EventReply::Card(Box::new(message))))
}

pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<RaidEvent>> {
    let (row, _) = load(&st, event_id, user.id).await?;
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    Ok(Json(ev))
}

#[derive(Deserialize)]
pub struct RangeQuery {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

/// Raids in an outhouse that you can see: upcoming ones (for the raid
/// list), or those overlapping `from`..`to` (for the calendar, up to 62 days).
pub async fn list_upcoming(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<Vec<RaidEvent>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let rows = match (q.from, q.to) {
        (Some(from), Some(to)) => {
            if to <= from || to - from > chrono::Duration::days(62) {
                return Err(AppError::BadRequest(
                    "Pick a range of up to 62 days.".into(),
                ));
            }
            let sql = format!(
                "SELECT {EVENT_COLUMNS} FROM events
                 WHERE guild_id = $1 AND starts_at < $3
                   AND starts_at + make_interval(mins => duration_minutes) > $2
                 ORDER BY starts_at, id LIMIT 300"
            );
            sqlx::query_as::<_, EventRow>(&sql)
                .bind(guild_id)
                .bind(from)
                .bind(to)
                .fetch_all(&st.db)
                .await?
        }
        (None, None) => {
            let sql = format!(
                "SELECT {EVENT_COLUMNS} FROM events
                 WHERE guild_id = $1
                   AND starts_at + make_interval(mins => duration_minutes) > now()
                 ORDER BY starts_at, id LIMIT 50"
            );
            sqlx::query_as::<_, EventRow>(&sql)
                .bind(guild_id)
                .fetch_all(&st.db)
                .await?
        }
        _ => {
            return Err(AppError::BadRequest(
                "Give both from and to, or neither.".into(),
            ))
        }
    };
    let mut seen: HashMap<i64, bool> = HashMap::new();
    let mut visible = Vec::new();
    for r in rows {
        let ok = match seen.get(&r.channel_id) {
            Some(&ok) => ok,
            None => {
                let ok = channel_access(&st.db, r.channel_id, user.id).await.is_ok();
                seen.insert(r.channel_id, ok);
                ok
            }
        };
        if ok {
            visible.push(r);
        }
    }
    Ok(Json(build(&st.db, visible).await?))
}

pub async fn update(
    State(st): State<AppState>,
    user: AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<UpdateEventRequest>,
) -> AppResult<Json<EventReply>> {
    let loot = check_loot(
        req.soft_reserves,
        req.reserve_lock.as_deref(),
        req.loot_instance.as_deref(),
    )?;
    let future = req.scope.as_deref() == Some("future");
    let reply = update_core(State(st.clone()), user, Path(event_id), Json(req)).await?;
    if !loot.any() {
        return Ok(reply);
    }
    // "Edit this and future raids" with new days may have moved this raid
    // into a new series (or cancelled it): follow it if it's still here.
    apply_loot(&st, event_id, future, &loot).await?;
    match load_row(&st.db, event_id).await {
        Ok(_) => Ok(Json(broadcast(&st, event_id).await?)),
        Err(AppError::NotFound) => Ok(reply),
        Err(e) => Err(e),
    }
}

async fn update_core(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<UpdateEventRequest>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    let future = match req.scope.as_deref() {
        None | Some("one") => false,
        Some("future") => true,
        Some(_) => return Err(AppError::BadRequest("Scope must be one or future.".into())),
    };
    let title = match req.title.as_deref() {
        Some(t) => clean_title(t)?,
        None => row.title.clone(),
    };
    let description = req
        .description
        .as_deref()
        .map(|d| clean_text(d, 1000))
        .unwrap_or_else(|| row.description.clone());
    let starts_at = req.starts_at.unwrap_or(row.starts_at);
    let moved = starts_at != row.starts_at;
    if moved && starts_at < Utc::now() - chrono::Duration::hours(1) {
        return Err(AppError::BadRequest("That time has already passed.".into()));
    }
    let duration = check_duration(req.duration_minutes)?.unwrap_or(row.duration_minutes);
    let tanks = check_slots(req.max_tanks)?.unwrap_or(row.max_tanks);
    let healers = check_slots(req.max_healers)?.unwrap_or(row.max_healers);
    let dps = check_slots(req.max_dps)?.unwrap_or(row.max_dps);
    let deadline =
        check_deadline(req.signup_deadline_minutes)?.unwrap_or(row.signup_deadline_minutes);
    let game = check_game(req.game.as_deref())?.unwrap_or_else(|| row.game.clone());
    let card_cleanup = check_card_cleanup(req.card_cleanup.as_deref(), row.series_id.is_some())?
        .unwrap_or_else(|| row.card_cleanup.clone());
    // A new posting rule (only raids whose card isn't up yet are affected).
    let series_tz: Option<String> = match row.series_id {
        Some(series_id) => {
            sqlx::query_scalar("SELECT time_zone FROM event_series WHERE id = $1")
                .bind(series_id)
                .fetch_optional(&st.db)
                .await?
        }
        None => None,
    };
    let rule_given = req.post_rule.is_some();
    let post_rule = check_post_rule(
        &st.db,
        req.post_rule.as_ref(),
        series_tz.as_deref().unwrap_or("UTC"),
    )
    .await?
    .map(DbJson);

    if let Some(repeat) = req.repeat.as_ref() {
        let details = NewDetails {
            title,
            description,
            starts_at,
            duration,
            tanks,
            healers,
            dps,
            deadline,
            game,
            card_cleanup,
            post_rule: if rule_given { Some(post_rule) } else { None },
        };
        if !future || row.series_id.is_none() {
            return Err(AppError::BadRequest(
                "Changing the repeat pattern applies to this and later raids of a repeating raid."
                    .into(),
            ));
        }
        return Ok(Json(
            change_pattern(&st, user.id, &row, repeat, details).await?,
        ));
    }

    let changed: Vec<i64> = match (future, row.series_id) {
        (true, Some(series_id)) => {
            let delta = (starts_at - row.starts_at).num_seconds() as f64;
            let ids: Vec<i64> = sqlx::query_scalar(
                "UPDATE events SET title = $3, description = $4,
                        starts_at = starts_at + make_interval(secs => $5),
                        duration_minutes = $6, max_tanks = $7, max_healers = $8, max_dps = $9,
                        signup_deadline_minutes = $10,
                        reminded = reminded AND NOT $11,
                        game = $12,
                        post_rule = CASE WHEN $13 THEN $14 ELSE post_rule END,
                        card_cleanup = $15
                 WHERE series_id = $1 AND starts_at >= $2
                 RETURNING id",
            )
            .bind(series_id)
            .bind(row.starts_at)
            .bind(&title)
            .bind(&description)
            .bind(delta)
            .bind(duration)
            .bind(tanks)
            .bind(healers)
            .bind(dps)
            .bind(deadline)
            .bind(moved)
            .bind(&game)
            .bind(rule_given)
            .bind(&post_rule)
            .bind(&card_cleanup)
            .fetch_all(&st.db)
            .await?;
            // Future raids the server hasn't made yet follow the new details too.
            if let Some(tz) = series_tz.clone() {
                let old_local = local_time_in(&st.db, row.starts_at, &tz).await?;
                let new_local = local_time_in(&st.db, starts_at, &tz).await?;
                let shift = (new_local.date() - old_local.date()).num_days();
                let rot = shift.rem_euclid(7) as u32;
                sqlx::query(
                    "UPDATE event_series SET title = $2, description = $3, duration_minutes = $4,
                            max_tanks = $5, max_healers = $6, max_dps = $7,
                            signup_deadline_minutes = $8, local_time = $9,
                            weekdays = (((weekdays << $10) | (weekdays >> (7 - $10))) & 127),
                            start_date = start_date + $11::int,
                            generated_through = generated_through + $11::int,
                            until_date = until_date + $11::int,
                            game = $12,
                            post_rule = CASE WHEN $13 THEN $14 ELSE post_rule END,
                            card_cleanup = $15
                     WHERE id = $1",
                )
                .bind(series_id)
                .bind(&title)
                .bind(&description)
                .bind(duration)
                .bind(tanks)
                .bind(healers)
                .bind(dps)
                .bind(deadline)
                .bind(new_local.time())
                .bind(rot as i32)
                .bind(shift as i32)
                .bind(&game)
                .bind(rule_given)
                .bind(&post_rule)
                .bind(&card_cleanup)
                .execute(&st.db)
                .await?;
            }
            ids
        }
        _ => {
            sqlx::query(
                "UPDATE events SET title = $2, description = $3, starts_at = $4,
                        duration_minutes = $5, max_tanks = $6, max_healers = $7, max_dps = $8,
                        signup_deadline_minutes = $9,
                        reminded = reminded AND NOT $10,
                        game = $11,
                        post_rule = CASE WHEN $12 THEN $13 ELSE post_rule END,
                        card_cleanup = $14
                 WHERE id = $1",
            )
            .bind(event_id)
            .bind(&title)
            .bind(&description)
            .bind(starts_at)
            .bind(duration)
            .bind(tanks)
            .bind(healers)
            .bind(dps)
            .bind(deadline)
            .bind(moved)
            .bind(&game)
            .bind(rule_given)
            .bind(&post_rule)
            .bind(&card_cleanup)
            .execute(&st.db)
            .await?;
            vec![event_id]
        }
    };
    // A new time or posting rule moves when unposted cards go up.
    refresh_post_times(&st, &changed).await?;
    // More slots may let people off the bench.
    for &id in &changed {
        let promoted = {
            let mut tx = st.db.begin().await?;
            let row = lock_event(&mut tx, id).await?;
            let promoted = fill_slots(&mut tx, &row).await?;
            tx.commit().await?;
            promoted
        };
        let row = load_row(&st.db, id).await?;
        notify_promoted(&st, &row, promoted).await;
        if id != event_id {
            broadcast(&st, id).await?;
        }
    }
    // Moving raids (or posting earlier) may make a card due: post it.
    post_due_cards(&st, None, Some(changed.as_slice())).await?;
    Ok(Json(broadcast(&st, event_id).await?))
}

// ---- Changing a repeating raid's pattern -------------------------------

/// The details "Edit this and future raids" saves along with a new pattern.
struct NewDetails {
    title: String,
    description: String,
    starts_at: DateTime<Utc>,
    duration: i32,
    tanks: i32,
    healers: i32,
    dps: i32,
    deadline: Option<i32>,
    game: String,
    card_cleanup: String,
    /// `None`: keep the series' rule. `Some(None)`: post straight away.
    post_rule: Option<Option<DbJson<PostRule>>>,
}

/// What a new pattern would do from a raid on (worked out without changing
/// anything, for the preview and for saving).
struct PatternPlan {
    series: SeriesRow,
    pat: Pattern,
    first_day: NaiveDate,
    local_time: NaiveTime,
    /// The series has no raids before this one: it's changed in place.
    in_place: bool,
    /// The chosen raid's day (in the series' time zone).
    from_day: NaiveDate,
    /// Existing raids that land on a day of the new pattern, by day.
    keep: HashMap<NaiveDate, i64>,
    kept_signups: i64,
    /// Existing raids that don't: cancelled when it's saved.
    removed: Vec<i64>,
    removed_signups: i64,
    new_raids: usize,
    next: Vec<DateTime<Utc>>,
    /// How many raids the series has left from this one on (if it has an end count).
    remaining: Option<i32>,
}

/// The pattern's next `n` start times from `first_day`, in UTC.
async fn next_starts(
    db: &PgPool,
    pat: &Pattern,
    first_day: NaiveDate,
    local_time: NaiveTime,
    n: usize,
) -> AppResult<Vec<DateTime<Utc>>> {
    let days = pattern_days(
        first_day,
        pat.mask,
        pat.every_weeks,
        pat.until,
        pat.count,
        first_day + chrono::Duration::days(3 * 366),
        n,
    );
    let mut out = Vec::with_capacity(days.len());
    for day in days {
        let at: DateTime<Utc> = sqlx::query_scalar("SELECT ($1::date + $2::time) AT TIME ZONE $3")
            .bind(day)
            .bind(local_time)
            .bind(&pat.time_zone)
            .fetch_one(db)
            .await?;
        out.push(at);
    }
    Ok(out)
}

/// `repeat: None` keeps the series' current pattern (with what's left of its count).
async fn plan_pattern(
    db: &PgPool,
    row: &EventRow,
    repeat: Option<&RepeatInfo>,
    starts_at: DateTime<Utc>,
) -> AppResult<PatternPlan> {
    let Some(series_id) = row.series_id else {
        return Err(AppError::BadRequest("This raid doesn't repeat.".into()));
    };
    let sql = format!("SELECT {SERIES_COLUMNS} FROM event_series WHERE id = $1");
    let series = sqlx::query_as::<_, SeriesRow>(&sql)
        .bind(series_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    let now = Utc::now();
    if row.starts_at <= now {
        return Err(AppError::BadRequest(
            "That raid has already started. Change the pattern from an upcoming raid.".into(),
        ));
    }
    if starts_at <= now {
        return Err(AppError::BadRequest("That time has already passed.".into()));
    }
    let tz = series.time_zone.clone();
    // The raids from this one on, with their day and how many signed up.
    let existing: Vec<(i64, NaiveDate, i64)> = sqlx::query_as(
        "SELECT e.id, (e.starts_at AT TIME ZONE $3)::date,
                (SELECT count(*) FROM event_signups s WHERE s.event_id = e.id AND s.status <> 'no')
         FROM events e
         WHERE e.series_id = $1 AND e.starts_at >= $2
         ORDER BY e.starts_at, e.id",
    )
    .bind(series_id)
    .bind(row.starts_at)
    .bind(&tz)
    .fetch_all(db)
    .await?;
    let earlier: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE series_id = $1 AND starts_at < $2)",
    )
    .bind(series_id)
    .bind(row.starts_at)
    .fetch_one(db)
    .await?;
    let remaining = series
        .max_count
        .map(|c| (c - (series.made_count - existing.len() as i32)).max(1));
    let info = match repeat {
        Some(r) => RepeatInfo {
            time_zone: tz.clone(),
            ..r.clone()
        },
        None => RepeatInfo {
            weekdays: mask_to_days(series.weekdays),
            every_weeks: series.every_weeks,
            until: series.until_date,
            count: remaining,
            time_zone: tz.clone(),
        },
    };
    if !(1..=4).contains(&info.every_weeks) && repeat.is_some() {
        return Err(AppError::BadRequest(
            "It can repeat every week or every 2, 3 or 4 weeks.".into(),
        ));
    }
    let pat = check_pattern(db, &info).await?;
    let local = local_time_in(db, starts_at, &tz).await?;
    let first_day = local.date();
    if pat.until.is_some_and(|u| u < first_day) {
        return Err(AppError::BadRequest(
            "The end date is before the first raid.".into(),
        ));
    }
    if pat
        .until
        .is_some_and(|u| u > first_day + chrono::Duration::days(731))
    {
        return Err(AppError::BadRequest(
            "A series can run for up to two years.".into(),
        ));
    }
    let from_day = local_time_in(db, row.starts_at, &tz).await?.date();
    let horizon = (now + chrono::Duration::days(AHEAD_DAYS + 1)).date_naive();
    let mut days = pattern_days(
        first_day,
        pat.mask,
        pat.every_weeks,
        pat.until,
        pat.count,
        horizon,
        800,
    );
    if days.is_empty() {
        days = pattern_days(
            first_day,
            pat.mask,
            pat.every_weeks,
            pat.until,
            pat.count,
            first_day + chrono::Duration::days(800),
            1,
        );
    }
    if days.is_empty() {
        return Err(AppError::BadRequest(
            "That pattern doesn't have any raids in it.".into(),
        ));
    }
    let mut keep = HashMap::new();
    let (mut kept_signups, mut removed_signups) = (0, 0);
    let mut removed = Vec::new();
    for (id, day, signups) in existing {
        if days.contains(&day) && !keep.contains_key(&day) {
            keep.insert(day, id);
            kept_signups += signups;
        } else {
            removed.push(id);
            removed_signups += signups;
        }
    }
    let new_raids = days.len() - keep.len();
    let next = next_starts(db, &pat, first_day, local.time(), 4).await?;
    Ok(PatternPlan {
        series,
        first_day,
        local_time: local.time(),
        in_place: !earlier,
        from_day,
        keep,
        kept_signups,
        removed,
        removed_signups,
        new_raids,
        next,
        remaining,
        pat,
    })
}

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Weekly on Tue & Thu" / "Every 2 weeks on Sun".
fn pattern_label(mask: i32, every_weeks: i32) -> String {
    let names: Vec<&str> = (0..7)
        .filter(|d| mask & (1 << d) != 0)
        .map(|d| WEEKDAY_NAMES[d as usize])
        .collect();
    let days = match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        n => format!("{} & {}", names[..n - 1].join(", "), names[n - 1]),
    };
    if every_weeks == 1 {
        format!("Weekly on {days}")
    } else {
        format!("Every {every_weeks} weeks on {days}")
    }
}

#[derive(Serialize)]
pub struct PatternPreview {
    /// The next (up to) four raids under the pattern.
    next: Vec<DateTime<Utc>>,
    /// Raids from this one on that would be cancelled, and their sign-ups.
    removed_raids: usize,
    removed_signups: i64,
    /// Raids that stay (same day), keeping their sign-ups.
    kept_raids: usize,
    kept_signups: i64,
    /// Raids that would be made (up to four weeks ahead).
    new_raids: usize,
    /// The pattern as it would be saved.
    repeat: RepeatInfo,
    label: String,
    /// How many raids the series has left from this one on (end count only).
    remaining_count: Option<i32>,
    /// No raids before this one: the series is changed rather than split.
    first_of_series: bool,
}

/// `POST /api/events/{id}/repeat-preview` (same body as editing "this and
/// future raids"): what a new pattern would do, without saving it. With no
/// `repeat` it describes the current pattern from this raid on.
pub async fn repeat_preview(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<UpdateEventRequest>,
) -> AppResult<Json<PatternPreview>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    let starts_at = req.starts_at.unwrap_or(row.starts_at);
    let plan = plan_pattern(&st.db, &row, req.repeat.as_ref(), starts_at).await?;
    Ok(Json(PatternPreview {
        next: plan.next.clone(),
        removed_raids: plan.removed.len(),
        removed_signups: plan.removed_signups,
        kept_raids: plan.keep.len(),
        kept_signups: plan.kept_signups,
        new_raids: plan.new_raids,
        label: pattern_label(plan.pat.mask, plan.pat.every_weeks),
        repeat: RepeatInfo {
            weekdays: mask_to_days(plan.pat.mask),
            every_weeks: plan.pat.every_weeks,
            until: plan.pat.until,
            count: plan.pat.count,
            time_zone: plan.pat.time_zone.clone(),
        },
        remaining_count: plan.remaining,
        first_of_series: plan.in_place,
    }))
}

/// Saves a new pattern from a raid on: the series is changed in place (if
/// this is its first raid) or split here. Later raids on a day the new
/// pattern still has stay, with their sign-ups (and take the new time and
/// details); the others are cancelled and the people signed up get a DM
/// from the Outhouse Helper. Earlier raids aren't touched.
async fn change_pattern(
    st: &AppState,
    user_id: i64,
    row: &EventRow,
    repeat: &RepeatInfo,
    d: NewDetails,
) -> AppResult<EventReply> {
    let plan = plan_pattern(&st.db, row, Some(repeat), d.starts_at).await?;
    let s = &plan.series;
    let post_rule = match d.post_rule {
        Some(rule) => rule,
        None => s.post_rule.clone(),
    };
    // Who to tell about cancelled raids (before their sign-ups go).
    let told: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT event_id, user_id FROM event_signups
         WHERE event_id = ANY($1) AND status <> 'no' ORDER BY event_id, signed_at",
    )
    .bind(&plan.removed)
    .fetch_all(&st.db)
    .await?;
    let removed_rows: Vec<EventRow> = {
        let sql = format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = ANY($1)");
        sqlx::query_as::<_, EventRow>(&sql)
            .bind(&plan.removed)
            .fetch_all(&st.db)
            .await?
    };

    let mut tx = st.db.begin().await?;
    let target = if plan.in_place {
        sqlx::query(
            "UPDATE event_series SET title = $2, description = $3, duration_minutes = $4,
                    max_tanks = $5, max_healers = $6, max_dps = $7, signup_deadline_minutes = $8,
                    start_date = $9, local_time = $10, weekdays = $11, every_weeks = $12,
                    until_date = $13, max_count = $14, made_count = 0, generated_through = NULL,
                    ended = false, post_rule = $15, game = $16, card_cleanup = $17
             WHERE id = $1",
        )
        .bind(s.id)
        .bind(&d.title)
        .bind(&d.description)
        .bind(d.duration)
        .bind(d.tanks)
        .bind(d.healers)
        .bind(d.dps)
        .bind(d.deadline)
        .bind(plan.first_day)
        .bind(plan.local_time)
        .bind(plan.pat.mask)
        .bind(plan.pat.every_weeks)
        .bind(plan.pat.until)
        .bind(plan.pat.count)
        .bind(&post_rule)
        .bind(&d.game)
        .bind(&d.card_cleanup)
        .execute(&mut *tx)
        .await?;
        s.id
    } else {
        // The old part ends the day before; its earlier raids stay as they are.
        let last_old = plan.from_day.min(plan.first_day) - chrono::Duration::days(1);
        sqlx::query(
            "UPDATE event_series SET ended = true,
                    until_date = LEAST(COALESCE(until_date, $2), $2)
             WHERE id = $1",
        )
        .bind(s.id)
        .bind(last_old)
        .execute(&mut *tx)
        .await?;
        let id = st.ids.next_id();
        sqlx::query(
            "INSERT INTO event_series (id, guild_id, channel_id, creator_id, title, description,
                 duration_minutes, max_tanks, max_healers, max_dps, signup_deadline_minutes,
                 time_zone, start_date, local_time, weekdays, every_weeks, until_date, max_count,
                 post_rule, game, card_cleanup, split_from)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                     $18, $19, $20, $21, $22)",
        )
        .bind(id)
        .bind(s.guild_id)
        .bind(s.channel_id)
        .bind(s.creator_id)
        .bind(&d.title)
        .bind(&d.description)
        .bind(d.duration)
        .bind(d.tanks)
        .bind(d.healers)
        .bind(d.dps)
        .bind(d.deadline)
        .bind(&s.time_zone)
        .bind(plan.first_day)
        .bind(plan.local_time)
        .bind(plan.pat.mask)
        .bind(plan.pat.every_weeks)
        .bind(plan.pat.until)
        .bind(plan.pat.count)
        .bind(&post_rule)
        .bind(&d.game)
        .bind(&d.card_cleanup)
        .bind(s.id)
        .execute(&mut *tx)
        .await?;
        id
    };
    let (new_ids, adopted) = materialise_tx(st, &mut tx, target, true, &plan.keep).await?;
    // Raids the new pattern doesn't have (sign-ups go with them).
    let gone: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "DELETE FROM events WHERE id = ANY($1) AND starts_at > now() RETURNING id, message_id",
    )
    .bind(&plan.removed)
    .fetch_all(&mut *tx)
    .await?;
    let cards: Vec<i64> = gone.iter().filter_map(|g| g.1).collect();
    sqlx::query("DELETE FROM messages WHERE id = ANY($1)")
        .bind(&cards)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let audience = Audience::Channel {
        guild_id: s.guild_id,
        channel_id: s.channel_id,
    };
    for (id, message_id) in &gone {
        if let Some(message_id) = message_id {
            st.dispatch(
                audience.clone(),
                ServerEvent::MessageDelete {
                    id: *message_id,
                    channel_id: s.channel_id,
                },
            );
        }
        st.dispatch(
            audience.clone(),
            ServerEvent::EventDelete {
                id: *id,
                guild_id: s.guild_id,
            },
        );
    }
    dispatch_events(st, &new_ids).await;
    let label = pattern_label(plan.pat.mask, plan.pat.every_weeks);
    let gone_ids: Vec<i64> = gone.iter().map(|g| g.0).collect();
    for r in removed_rows.iter().filter(|r| gone_ids.contains(&r.id)) {
        for (_, uid) in told.iter().filter(|(e, _)| *e == r.id) {
            if let Err(e) = dm_moved(st, r, *uid, &s.time_zone, &label).await {
                tracing::warn!(error = %e, "couldn't DM someone about a moved raid");
            }
        }
    }
    // Kept raids: new time and details; posting times and the bench follow.
    refresh_post_times(st, &adopted).await?;
    for &id in &adopted {
        let promoted = {
            let mut tx = st.db.begin().await?;
            let r = lock_event(&mut tx, id).await?;
            let promoted = fill_slots(&mut tx, &r).await?;
            tx.commit().await?;
            promoted
        };
        let r = load_row(&st.db, id).await?;
        notify_promoted(st, &r, promoted).await;
        broadcast(st, id).await?;
    }
    let mut touched = new_ids.clone();
    touched.extend(&adopted);
    post_due_cards(st, None, Some(touched.as_slice())).await?;

    let old_label = pattern_label(s.weekdays, s.every_weeks);
    crate::audit::Entry::new(s.guild_id, user_id, "raid_repeat_update")
        .target("raid", row.id, d.title.clone())
        .details(format!(
            "{old_label} → {label}, from {}: {} raid{} cancelled ({} sign-up{} dropped), {} kept, {} new",
            plan.first_day.min(plan.from_day).format("%a %-d %b %Y"),
            gone.len(),
            if gone.len() == 1 { "" } else { "s" },
            plan.removed_signups,
            if plan.removed_signups == 1 { "" } else { "s" },
            adopted.len(),
            new_ids.len(),
        ))
        .save(st)
        .await;

    let first: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM events WHERE series_id = $1 AND starts_at >= $2 ORDER BY starts_at, id LIMIT 1",
    )
    .bind(target)
    .bind(Utc::now() - chrono::Duration::minutes(30))
    .fetch_optional(&st.db)
    .await?;
    let first = first
        .ok_or_else(|| AppError::BadRequest("That pattern doesn't have any raids in it.".into()))?;
    broadcast(st, first).await
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Tells someone the raid they signed up for is off (the pattern changed).
async fn dm_moved(
    st: &AppState,
    row: &EventRow,
    user_id: i64,
    tz: &str,
    label: &str,
) -> AppResult<()> {
    let (guild, channel): (String, String) = sqlx::query_as(
        "SELECT g.name, c.name FROM channels c JOIN guilds g ON g.id = c.guild_id WHERE c.id = $1",
    )
    .bind(row.channel_id)
    .fetch_one(&st.db)
    .await?;
    let bot = helper_bot(st, row.guild_id).await?;
    let dm = super::dms::open_direct(st, bot, user_id).await?;
    // The raid's time as a tag (each reader sees it in their own zone); the
    // repeat days are the planner's, so their zone is named.
    let content = format!(
        "📅 Heads up: **{}** on {} has been moved or cancelled. The raid now repeats {} ({tz} time), so your sign-up for that day was dropped. Check the calendar in #{channel} ({guild}) for the new dates and sign up again.",
        row.title,
        crate::timetag::tag(row.starts_at, 'F'),
        lower_first(label),
    );
    post_bot_message(
        st,
        outhouse_common::models::DM_GUILD,
        dm,
        bot,
        None,
        None,
        content,
        Vec::new(),
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct ScopeQuery {
    scope: Option<String>,
}

/// Cancels the raid (and its chat message); `?scope=future` also cancels
/// the rest of its series.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Query(q): Query<ScopeQuery>,
) -> AppResult<StatusCode> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    let gone: Vec<(i64, Option<i64>)> = match (q.scope.as_deref(), row.series_id) {
        (Some("future"), Some(series_id)) => {
            sqlx::query("UPDATE event_series SET ended = true WHERE id = $1")
                .bind(series_id)
                .execute(&st.db)
                .await?;
            sqlx::query_as(
                "DELETE FROM events WHERE series_id = $1 AND starts_at >= $2
                 RETURNING id, message_id",
            )
            .bind(series_id)
            .bind(row.starts_at)
            .fetch_all(&st.db)
            .await?
        }
        (None | Some("one") | Some("future"), _) => {
            sqlx::query_as("DELETE FROM events WHERE id = $1 RETURNING id, message_id")
                .bind(event_id)
                .fetch_all(&st.db)
                .await?
        }
        _ => return Err(AppError::BadRequest("Scope must be one or future.".into())),
    };
    let audience = Audience::Channel {
        guild_id: row.guild_id,
        channel_id: row.channel_id,
    };
    crate::audit::Entry::new(row.guild_id, user.id, "raid_delete")
        .target("raid", event_id, row.title.clone())
        .details(audit_raid_details(&row, gone.len()))
        .save(&st)
        .await;
    for (id, message_id) in gone {
        if let Some(message_id) = message_id {
            sqlx::query("DELETE FROM messages WHERE id = $1")
                .bind(message_id)
                .execute(&st.db)
                .await?;
            st.dispatch(
                audience.clone(),
                ServerEvent::MessageDelete {
                    id: message_id,
                    channel_id: row.channel_id,
                },
            );
        }
        st.dispatch(
            audience.clone(),
            ServerEvent::EventDelete {
                id,
                guild_id: row.guild_id,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

fn audit_raid_details(row: &EventRow, count: usize) -> String {
    let when = row.starts_at.format("%a %-d %b %Y, %H:%M UTC");
    if count > 1 {
        format!(
            "This and {} later raids in the series (from {when})",
            count - 1
        )
    } else {
        format!("Was on {when}")
    }
}

/// Locks the raid's row for the rest of the transaction (so two people
/// can't both take the last spot) and returns its current state.
async fn lock_event(tx: &mut Transaction<'_, Postgres>, event_id: i64) -> AppResult<EventRow> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = $1 FOR UPDATE");
    sqlx::query_as::<_, EventRow>(&sql)
        .bind(event_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)
}

/// Moves people up from the bench (first benched, first in) while their
/// role has free spots. Only people the server benched are moved: someone a
/// raid leader benched stays put. Returns who moved up and as what.
async fn fill_slots(
    tx: &mut Transaction<'_, Postgres>,
    row: &EventRow,
) -> AppResult<Vec<(i64, String)>> {
    if row.finished() {
        return Ok(Vec::new());
    }
    let mut promoted = Vec::new();
    for role in ["tank", "healer", "dps"] {
        let taken: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event_signups
             WHERE event_id = $1 AND role = $2 AND status IN ('yes', 'late')",
        )
        .bind(row.id)
        .bind(role)
        .fetch_one(&mut **tx)
        .await?;
        let free = row.max_for(role) as i64 - taken;
        if free <= 0 {
            continue;
        }
        let ids: Vec<i64> = sqlx::query_scalar(
            "UPDATE event_signups SET status = bench_wanted, bench_wanted = NULL
             WHERE event_id = $1 AND user_id IN (
                 SELECT user_id FROM event_signups
                 WHERE event_id = $1 AND role = $2 AND status = 'bench'
                   AND bench_wanted IS NOT NULL
                 ORDER BY signed_at, user_id
                 LIMIT $3)
             RETURNING user_id",
        )
        .bind(row.id)
        .bind(role)
        .bind(free)
        .fetch_all(&mut **tx)
        .await?;
        promoted.extend(ids.into_iter().map(|id| (id, role.to_string())));
    }
    Ok(promoted)
}

fn role_name(role: &str) -> &'static str {
    match role {
        "tank" => "a tank",
        "healer" => "a healer",
        _ => "DPS",
    }
}

/// Tells people they've come off the bench: a live notice, plus a DM from
/// the outhouse's helper bot.
async fn notify_promoted(st: &AppState, row: &EventRow, promoted: Vec<(i64, String)>) {
    for (user_id, role) in promoted {
        st.dispatch(
            Audience::User(user_id),
            ServerEvent::RaidPromoted {
                event_id: row.id,
                guild_id: row.guild_id,
                channel_id: row.channel_id,
                title: row.title.clone(),
                role: role.clone(),
            },
        );
        if let Err(e) = dm_promoted(st, row, user_id, &role).await {
            tracing::warn!(error = %e, "couldn't DM a promoted raider");
        }
    }
}

async fn dm_promoted(st: &AppState, row: &EventRow, user_id: i64, role: &str) -> AppResult<()> {
    let (guild, channel): (String, String) = sqlx::query_as(
        "SELECT g.name, c.name FROM channels c JOIN guilds g ON g.id = c.guild_id WHERE c.id = $1",
    )
    .bind(row.channel_id)
    .fetch_one(&st.db)
    .await?;
    let bot = helper_bot(st, row.guild_id).await?;
    let dm = super::dms::open_direct(st, bot, user_id).await?;
    let content = format!(
        "🎉 Good news! A spot opened up in **{}** on {} (#{channel} in {guild}), so you're off the bench and on the roster as {}. See you there!",
        row.title,
        crate::timetag::tag(row.starts_at, 'F'),
        role_name(role)
    );
    post_bot_message(
        st,
        outhouse_common::models::DM_GUILD,
        dm,
        bot,
        None,
        None,
        content,
        Vec::new(),
    )
    .await?;
    Ok(())
}

const LOCKED: &str =
    "Sign-ups for this raid have closed. Ask the raid leader if you need a change.";
const NOT_OPEN: &str =
    "Sign-ups for this raid aren't open yet. They open when its sign-up post goes up in chat.";

/// Signs up (or changes your sign-up). If your role is full you go on the
/// bench and move up when a spot opens. Raid leaders can pass `user_id` to
/// sign up or move someone else (that skips the bench), and can still make
/// changes after the sign-up deadline.
pub async fn signup(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<SignupRequest>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let manager = can_manage(&st.db, &row, &access).await?;
    let target = req.user_id.unwrap_or(user.id);
    let on_behalf = target != user.id;
    if on_behalf {
        if !manager {
            return Err(AppError::Forbidden);
        }
        // They must be in the outhouse.
        member_ctx(&st.db, row.guild_id, target).await?;
    } else if access.ctx.timed_out {
        return Err(AppError::Forbidden);
    }
    if !manager && row.not_open() {
        return Err(AppError::BadRequest(NOT_OPEN.into()));
    }
    if !manager && row.locked() {
        return Err(AppError::BadRequest(LOCKED.into()));
    }
    if !STATUSES.contains(&req.status.as_str()) {
        return Err(AppError::BadRequest("Unknown sign-up status.".into()));
    }
    if !ROLES.contains(&req.role.as_str()) {
        return Err(AppError::BadRequest(
            "Role must be tank, healer or dps.".into(),
        ));
    }
    let needs_role = matches!(req.status.as_str(), "yes" | "late" | "tentative" | "bench");
    if needs_role && req.role.is_empty() {
        return Err(AppError::BadRequest("Pick tank, healer or DPS.".into()));
    }
    // The character they're bringing: copied onto the sign-up.
    let character: Option<CharacterCopy> = match req.character_id {
        Some(character_id) => Some(character_copy(&st.db, character_id, target, &row.game).await?),
        None => None,
    };
    let set_character = !req.keep_character || character.is_some();
    let mut class = clean_text(&req.class, 32);
    if class.is_empty() {
        if let Some(c) = &character {
            class = c.class.clone();
        }
    }

    let mut tx = st.db.begin().await?;
    let row = lock_event(&mut tx, event_id).await?;
    let prev: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT status, role, bench_wanted FROM event_signups WHERE event_id = $1 AND user_id = $2",
    )
    .bind(event_id)
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    let mut status = req.status.clone();
    let mut bench_wanted: Option<String> = None;
    if matches!(status.as_str(), "yes" | "late") && !on_behalf {
        let same_role = prev.as_ref().is_some_and(|(_, r, _)| *r == req.role);
        let on_roster = same_role
            && prev
                .as_ref()
                .is_some_and(|(s, _, _)| s == "yes" || s == "late");
        let waiting = same_role && prev.as_ref().is_some_and(|(_, _, w)| w.is_some());
        if waiting {
            // Still in the queue: keep your place.
            status = "bench".into();
            bench_wanted = Some(req.status.clone());
        } else if !on_roster {
            let taken: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM event_signups
                 WHERE event_id = $1 AND role = $2 AND status IN ('yes', 'late') AND user_id <> $3",
            )
            .bind(event_id)
            .bind(&req.role)
            .bind(target)
            .fetch_one(&mut *tx)
            .await?;
            if taken >= row.max_for(&req.role) as i64 {
                status = "bench".into();
                bench_wanted = Some(req.status.clone());
            }
        }
    }
    let copy = character.as_ref();
    sqlx::query(
        "INSERT INTO event_signups (event_id, user_id, status, role, class, spec, note, bench_wanted,
                                    character_id, character_name, character_last_name,
                                    character_class, character_game)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (event_id, user_id) DO UPDATE
             SET status = EXCLUDED.status, role = EXCLUDED.role, class = EXCLUDED.class,
                 spec = EXCLUDED.spec, note = EXCLUDED.note, bench_wanted = EXCLUDED.bench_wanted,
                 character_id = CASE WHEN $14 THEN EXCLUDED.character_id
                                     ELSE event_signups.character_id END,
                 character_name = CASE WHEN $14 THEN EXCLUDED.character_name
                                       ELSE event_signups.character_name END,
                 character_last_name = CASE WHEN $14 THEN EXCLUDED.character_last_name
                                            ELSE event_signups.character_last_name END,
                 character_class = CASE WHEN $14 THEN EXCLUDED.character_class
                                        ELSE event_signups.character_class END,
                 character_game = CASE WHEN $14 THEN EXCLUDED.character_game
                                       ELSE event_signups.character_game END,
                 -- Changing your mind keeps your place unless you were out
                 -- or you're joining a different role's queue.
                 signed_at = CASE
                     WHEN event_signups.status = 'no' AND EXCLUDED.status <> 'no' THEN now()
                     WHEN EXCLUDED.bench_wanted IS NOT NULL
                          AND (event_signups.bench_wanted IS NULL
                               OR event_signups.role <> EXCLUDED.role) THEN now()
                     ELSE event_signups.signed_at END",
    )
    .bind(event_id)
    .bind(target)
    .bind(&status)
    .bind(&req.role)
    .bind(&class)
    .bind(clean_text(&req.spec, 32))
    .bind(clean_text(&req.note, 100))
    .bind(&bench_wanted)
    .bind(copy.map(|c| c.id))
    .bind(copy.map(|c| c.name.as_str()).unwrap_or(""))
    .bind(copy.map(|c| c.last_name.as_str()).unwrap_or(""))
    .bind(copy.map(|c| c.class.as_str()).unwrap_or(""))
    .bind(copy.map(|c| c.game.as_str()).unwrap_or(""))
    .bind(set_character)
    .execute(&mut *tx)
    .await?;
    let promoted = fill_slots(&mut tx, &row).await?;
    tx.commit().await?;
    notify_promoted(&st, &row, promoted).await;
    Ok(Json(broadcast(&st, event_id).await?))
}

/// A character picked for a sign-up, as it is right now.
struct CharacterCopy {
    id: i64,
    name: String,
    last_name: String,
    class: String,
    game: String,
}

/// Checks the character belongs to the person signing up (and suits the
/// raid's game) and copies its name and class.
async fn character_copy(
    db: &PgPool,
    character_id: i64,
    owner: i64,
    raid_game: &str,
) -> AppResult<CharacterCopy> {
    let found: Option<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT user_id, name, last_name, class, game FROM wow_characters WHERE id = $1",
    )
    .bind(character_id)
    .fetch_optional(db)
    .await?;
    let Some((_, name, last_name, class, game)) = found.filter(|f| f.0 == owner) else {
        return Err(AppError::BadRequest(
            "That character isn't one of theirs. Pick another, or sign up without one.".into(),
        ));
    };
    if !raid_game.is_empty() && game != raid_game {
        return Err(AppError::BadRequest(format!(
            "This raid is for {}: pick a character from that game.",
            outhouse_common::wow::game_label(raid_game)
        )));
    }
    Ok(CharacterCopy {
        id: character_id,
        name,
        last_name,
        class,
        game,
    })
}

/// Raid leaders' list of who to invite (`GET /api/events/{id}/export`):
/// everyone signed up except "can't make it", roster first (tanks, healers,
/// DPS), then tentative, then the bench, each in sign-up order. The client
/// turns it into invite lists and macros.
pub async fn export(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<RaidExport>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&st.db, &row, &access).await? {
        return Err(AppError::Forbidden);
    }
    let (title, starts_at, game) = (row.title.clone(), row.starts_at, row.game.clone());
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    let status_rank = |s: &str| match s {
        "yes" | "late" => 0,
        "tentative" => 1,
        _ => 2,
    };
    let role_rank = |r: &str| match r {
        "tank" => 0,
        "healer" => 1,
        "dps" => 2,
        _ => 3,
    };
    let mut signups: Vec<EventSignup> = ev
        .signups
        .into_iter()
        .filter(|s| s.status != "no")
        .collect();
    // Stable: sign-up order is kept within each group.
    signups.sort_by_key(|s| (status_rank(s.status.as_str()), role_rank(s.role.as_str())));
    Ok(Json(RaidExport {
        event_id,
        title,
        starts_at,
        game,
        entries: signups
            .into_iter()
            .map(|s| RaidExportEntry {
                user_id: s.user_id,
                display_name: s.display_name,
                status: s.status,
                role: s.role,
                class: s.class,
                spec: s.spec,
                character: s.character,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct CompositionQuery {
    extra: Option<String>,
}

/// `GET /api/events/{id}/composition[?extra=1]`: roles against slots,
/// classes, armour, and which raid buffs, debuffs and dispels the roster
/// brings (with `extra=1`, tentative and bench count too). Anyone who can
/// see the raid can look.
pub async fn composition(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Query(q): Query<CompositionQuery>,
) -> AppResult<Json<outhouse_common::raidplan::Composition>> {
    use outhouse_common::raidplan::{compose, CompMember};
    let (row, _) = load(&st, event_id, user.id).await?;
    let extra = matches!(q.extra.as_deref(), Some("1" | "true" | "yes"));
    let raid_game = row.game.clone();
    let slots = [
        ("tank", row.max_tanks),
        ("healer", row.max_healers),
        ("dps", row.max_dps),
    ];
    let ev = build(&st.db, vec![row])
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    let counted: Vec<&EventSignup> = ev
        .signups
        .iter()
        .filter(|s| match s.status.as_str() {
            "yes" | "late" => true,
            "tentative" | "bench" => extra,
            _ => false,
        })
        .collect();
    // Which buffs to check: the raid's game, else the game most of the
    // characters signed up are from.
    let game = if raid_game.is_empty() {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for s in &counted {
            if let Some(c) = &s.character {
                *counts.entry(c.game.as_str()).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|&(g, n)| (n, g == "tbc"))
            .map(|(g, _)| g.to_string())
            .unwrap_or_default()
    } else {
        raid_game
    };
    let members: Vec<CompMember> = counted
        .iter()
        .map(|s| CompMember {
            user_id: s.user_id,
            name: s
                .character
                .as_ref()
                .map(|c| c.full_name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| s.display_name.clone()),
            class: s.class.clone(),
            spec: s.spec.clone(),
            role: s.role.clone(),
        })
        .collect();
    Ok(Json(compose(event_id, &members, &game, slots, extra)))
}

/// Removes a sign-up (yours, or anyone's if you lead the raid). The first
/// person benched for that role moves up.
pub async fn withdraw(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((event_id, target)): Path<(i64, i64)>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let manager = can_manage(&st.db, &row, &access).await?;
    if target != user.id && !manager {
        return Err(AppError::Forbidden);
    }
    if !manager && row.not_open() {
        return Err(AppError::BadRequest(NOT_OPEN.into()));
    }
    if !manager && row.locked() {
        return Err(AppError::BadRequest(LOCKED.into()));
    }
    let mut tx = st.db.begin().await?;
    let row = lock_event(&mut tx, event_id).await?;
    sqlx::query("DELETE FROM event_signups WHERE event_id = $1 AND user_id = $2")
        .bind(event_id)
        .bind(target)
        .execute(&mut *tx)
        .await?;
    let promoted = fill_slots(&mut tx, &row).await?;
    tx.commit().await?;
    notify_promoted(&st, &row, promoted).await;
    Ok(Json(broadcast(&st, event_id).await?))
}

// ---- Reminders ---------------------------------------------------------

/// The outhouse's helper bot (posts reminders), made on first use.
pub async fn helper_bot(st: &AppState, guild_id: i64) -> AppResult<i64> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT helper_bot_id FROM guilds WHERE id = $1")
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?
            .flatten();
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    auth::create_bot_user(&mut *tx, id, "Outhouse Helper").await?;
    sqlx::query("UPDATE guilds SET helper_bot_id = $2 WHERE id = $1 AND helper_bot_id IS NULL")
        .bind(guild_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // Another reminder may have raced us; use whichever won.
    let winner: Option<i64> = sqlx::query_scalar("SELECT helper_bot_id FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    Ok(winner.unwrap_or(id))
}

/// Every minute: make upcoming raids for repeating series, post due
/// cards, and post a reminder for raids starting within 15 minutes.
pub fn spawn_reminders(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = run_series(&st).await {
                tracing::warn!(error = %e, "repeating raids failed");
            }
            if let Err(e) = send_due_reminders(&st).await {
                tracing::warn!(error = %e, "raid reminders failed");
            }
        }
    });
}

async fn send_due_reminders(st: &AppState) -> AppResult<()> {
    // Claim them first so a reminder never goes out twice.
    let sql = format!(
        "UPDATE events SET reminded = true
         WHERE NOT reminded
           AND starts_at <= now() + make_interval(mins => {REMIND_MINUTES})
           AND starts_at > now() - interval '30 minutes'
         RETURNING {EVENT_COLUMNS}"
    );
    let due = sqlx::query_as::<_, EventRow>(&sql)
        .fetch_all(&st.db)
        .await?;
    for row in due {
        let people: Vec<i64> = sqlx::query_scalar(
            "SELECT user_id FROM event_signups
             WHERE event_id = $1 AND status IN ('yes', 'late', 'tentative')
             ORDER BY signed_at",
        )
        .bind(row.id)
        .fetch_all(&st.db)
        .await?;
        let minutes = (row.starts_at - Utc::now()).num_minutes().max(0);
        // A relative time tag keeps counting down in chat ("in 12 minutes",
        // then "3 minutes ago"), with the clock time in each reader's zone.
        let when = if minutes <= 1 {
            "is starting now".to_string()
        } else {
            format!(
                "starts {} ({})",
                crate::timetag::tag(row.starts_at, 'R'),
                crate::timetag::tag(row.starts_at, 't')
            )
        };
        let mentions: Vec<String> = people.iter().map(|id| format!("<@{id}>")).collect();
        let content = if mentions.is_empty() {
            format!("⏰ **{}** {when}. Nobody has signed up yet!", row.title)
        } else {
            format!("⏰ **{}** {when}! {}", row.title, mentions.join(" "))
        };
        let bot = helper_bot(st, row.guild_id).await?;
        post_bot_message(
            st,
            row.guild_id,
            row.channel_id,
            bot,
            None,
            None,
            content,
            Vec::new(),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn pattern_days_follow_weeks_and_ends() {
        // Mon 2 Nov 2026, Tuesdays and Thursdays every second week.
        let mask = (1 << 1) | (1 << 3);
        let days = pattern_days(d(2026, 11, 2), mask, 2, None, None, d(2026, 11, 30), 100);
        assert_eq!(
            days,
            vec![
                d(2026, 11, 3),
                d(2026, 11, 5),
                d(2026, 11, 17),
                d(2026, 11, 19)
            ]
        );
        // Ends after three, or on a date.
        assert_eq!(
            pattern_days(d(2026, 11, 2), mask, 1, None, Some(3), d(2027, 1, 1), 100).len(),
            3
        );
        assert_eq!(
            pattern_days(
                d(2026, 11, 2),
                mask,
                1,
                Some(d(2026, 11, 10)),
                None,
                d(2027, 1, 1),
                100
            ),
            vec![d(2026, 11, 3), d(2026, 11, 5), d(2026, 11, 10)]
        );
        // The first day counts when it matches; a limit caps the list.
        assert_eq!(
            pattern_days(d(2026, 11, 3), 1 << 1, 1, None, None, d(2027, 1, 1), 2),
            vec![d(2026, 11, 3), d(2026, 11, 10)]
        );
    }

    #[test]
    fn labels_read_nicely() {
        assert_eq!(pattern_label(1 << 6, 1), "Weekly on Sun");
        assert_eq!(
            pattern_label((1 << 1) | (1 << 3), 2),
            "Every 2 weeks on Tue & Thu"
        );
        assert_eq!(lower_first("Weekly on Sun"), "weekly on Sun");
    }
}
