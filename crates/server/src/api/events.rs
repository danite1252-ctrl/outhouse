//! Raid planner: events people sign up for (tank / healer / DPS, class and
//! spec), shown as a live card in chat, with a reminder before they start.
//!
//! Extras: repeating raids (the server makes each raid a few weeks ahead so
//! it has its own sign-ups, and posts its card a week before), sign-up
//! deadlines, an automatic bench when a role is full (first in, first
//! promoted), and attendance (see `attendance.rs`).

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    CreateEventRequest, EventSignup, Message, RaidEvent, RepeatInfo, SignupRequest,
    UpdateEventRequest,
};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};

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
/// A repeating raid's chat card is posted this many days before it starts.
const CARD_DAYS: i64 = 7;
/// Longest sign-up deadline: a week before the start.
const MAX_DEADLINE_MINUTES: i32 = 7 * 24 * 60;

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
}

impl EventRow {
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
}

pub(crate) const EVENT_COLUMNS: &str = "id, guild_id, channel_id, message_id, creator_id, title,
    description, starts_at, duration_minutes, max_tanks, max_healers, max_dps,
    series_id, signup_deadline_minutes, attendance_taken";

const SERIES_COLUMNS: &str = "id, guild_id, channel_id, creator_id, title, description,
    duration_minutes, max_tanks, max_healers, max_dps, signup_deadline_minutes, time_zone,
    start_date, local_time, weekdays, every_weeks, until_date, max_count, made_count,
    generated_through";

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
                s.status, s.role, s.class, s.spec, s.note, s.signed_at, s.bench_wanted
         FROM event_signups s
         JOIN events e ON e.id = s.event_id
         JOIN users u ON u.id = s.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = s.user_id
         WHERE s.event_id = ANY($1)
         ORDER BY s.signed_at, s.user_id",
    )
    .bind(event_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<EventSignup>> = HashMap::new();
    for r in rows {
        map.entry(r.event_id).or_default().push(EventSignup {
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
    Ok(rows
        .into_iter()
        .map(|r| {
            let closes = r.closes_at();
            RaidEvent {
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

/// Raid leaders: whoever planned it, and anyone with Manage Events.
pub(crate) fn can_manage(row: &EventRow, access: &ChannelAccess) -> bool {
    row.creator_id == access.ctx.user_id || access.perms & p::MANAGE_EVENTS != 0
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
    let tz = r.time_zone.trim().to_string();
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name = $1)")
            .bind(&tz)
            .fetch_one(db)
            .await?;
    if tz.is_empty() || tz.len() > 64 || !known {
        return Err(AppError::BadRequest("Unknown time zone.".into()));
    }
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
    let sql =
        format!("SELECT {SERIES_COLUMNS} FROM event_series WHERE id = $1 AND NOT ended FOR UPDATE");
    let Some(s) = sqlx::query_as::<_, SeriesRow>(&sql)
        .bind(series_id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(Vec::new());
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
    // At most every 8 weeks: a match always turns up within 56 days.
    for _ in 0..800 {
        if day > horizon && !(at_least_one && new_ids.is_empty()) {
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
                    .fetch_one(&mut *tx)
                    .await?;
            // Skip ones already over (e.g. a series that started today, earlier on).
            if starts_at > now - chrono::Duration::minutes(30) {
                let id = st.ids.next_id();
                sqlx::query(
                    "INSERT INTO events (id, guild_id, channel_id, message_id, creator_id, title,
                         description, starts_at, duration_minutes, max_tanks, max_healers,
                         max_dps, series_id, signup_deadline_minutes)
                     VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
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
                .execute(&mut *tx)
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
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    for &id in &new_ids {
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
    Ok(new_ids)
}

/// Posts the chat card for raids starting within [`CARD_DAYS`] that don't
/// have one yet (only this series' raids if given).
async fn post_due_cards(st: &AppState, series_id: Option<i64>) -> AppResult<()> {
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM events
         WHERE message_id IS NULL
           AND starts_at <= now() + make_interval(days => {CARD_DAYS})
           AND starts_at + make_interval(mins => duration_minutes) > now()
           AND ($1::BIGINT IS NULL OR series_id = $1)
         ORDER BY starts_at, id
         LIMIT 100"
    );
    let rows = sqlx::query_as::<_, EventRow>(&sql)
        .bind(series_id)
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
        let claimed =
            sqlx::query("UPDATE events SET message_id = $2 WHERE id = $1 AND message_id IS NULL")
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
        st.dispatch(
            Audience::Channel {
                guild_id: row.guild_id,
                channel_id: row.channel_id,
            },
            ServerEvent::MessageCreate(message),
        );
    }
    Ok(())
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
    post_due_cards(st, None).await
}

// ---- Endpoints ---------------------------------------------------------

/// Posts a new raid in a channel (or starts a repeating series).
pub async fn create(
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
    let title = clean_title(&req.title)?;
    let description = clean_text(&req.description, 1000);
    let duration = check_duration(req.duration_minutes)?.unwrap_or(180);
    // Defaults: a 40-player raid.
    let tanks = check_slots(req.max_tanks)?.unwrap_or(5);
    let healers = check_slots(req.max_healers)?.unwrap_or(10);
    let dps = check_slots(req.max_dps)?.unwrap_or(25);
    let deadline = check_deadline(req.signup_deadline_minutes)?.flatten();
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
                 time_zone, start_date, local_time, weekdays, every_weeks, until_date, max_count)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
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
        post_due_cards(&st, Some(series_id)).await?;
        return Ok(Json(broadcast(&st, first).await?));
    }

    let message_id = st.ids.next_id();
    let event_id = st.ids.next_id();
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
                             signup_deadline_minutes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
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
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<UpdateEventRequest>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&row, &access) {
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

    let changed: Vec<i64> = match (future, row.series_id) {
        (true, Some(series_id)) => {
            let delta = (starts_at - row.starts_at).num_seconds() as f64;
            let ids: Vec<i64> = sqlx::query_scalar(
                "UPDATE events SET title = $3, description = $4,
                        starts_at = starts_at + make_interval(secs => $5),
                        duration_minutes = $6, max_tanks = $7, max_healers = $8, max_dps = $9,
                        signup_deadline_minutes = $10,
                        reminded = reminded AND NOT $11
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
            .fetch_all(&st.db)
            .await?;
            // Future raids the server hasn't made yet follow the new details too.
            let tz: Option<String> =
                sqlx::query_scalar("SELECT time_zone FROM event_series WHERE id = $1")
                    .bind(series_id)
                    .fetch_optional(&st.db)
                    .await?;
            if let Some(tz) = tz {
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
                            until_date = until_date + $11::int
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
                        reminded = reminded AND NOT $10
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
            .execute(&st.db)
            .await?;
            vec![event_id]
        }
    };
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
    // Moving raids may bring one within a week: post its card.
    if let Some(series_id) = row.series_id {
        post_due_cards(&st, Some(series_id)).await?;
    }
    Ok(Json(broadcast(&st, event_id).await?))
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
    if !can_manage(&row, &access) {
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
        "🎉 Good news! A spot opened up in **{}** (#{channel} in {guild}), so you're off the bench and on the roster as {}. See you there!",
        row.title,
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
    let manager = can_manage(&row, &access);
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
    sqlx::query(
        "INSERT INTO event_signups (event_id, user_id, status, role, class, spec, note, bench_wanted)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (event_id, user_id) DO UPDATE
             SET status = EXCLUDED.status, role = EXCLUDED.role, class = EXCLUDED.class,
                 spec = EXCLUDED.spec, note = EXCLUDED.note, bench_wanted = EXCLUDED.bench_wanted,
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
    .bind(clean_text(&req.class, 32))
    .bind(clean_text(&req.spec, 32))
    .bind(clean_text(&req.note, 100))
    .bind(&bench_wanted)
    .execute(&mut *tx)
    .await?;
    let promoted = fill_slots(&mut tx, &row).await?;
    tx.commit().await?;
    notify_promoted(&st, &row, promoted).await;
    Ok(Json(broadcast(&st, event_id).await?))
}

/// Removes a sign-up (yours, or anyone's if you lead the raid). The first
/// person benched for that role moves up.
pub async fn withdraw(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((event_id, target)): Path<(i64, i64)>,
) -> AppResult<Json<EventReply>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let manager = can_manage(&row, &access);
    if target != user.id && !manager {
        return Err(AppError::Forbidden);
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
        let when = if minutes <= 1 {
            "is starting now".to_string()
        } else {
            format!("starts in {minutes} minutes")
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
