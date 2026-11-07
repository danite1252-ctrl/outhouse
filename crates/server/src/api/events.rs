//! Raid planner: events people sign up for (tank / healer / DPS, class and
//! spec), shown as a live card in chat, with a reminder before they start.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    CreateEventRequest, EventSignup, Message, RaidEvent, SignupRequest, UpdateEventRequest,
};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use super::messages::{fetch_message, post_bot_message};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx, ChannelAccess};
use crate::state::{AppState, Audience};

const STATUSES: &[&str] = &["yes", "late", "tentative", "bench", "no"];
const ROLES: &[&str] = &["tank", "healer", "dps", ""];
/// How long before the start the reminder goes out.
const REMIND_MINUTES: i64 = 15;

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    guild_id: i64,
    channel_id: i64,
    message_id: i64,
    creator_id: i64,
    title: String,
    description: String,
    starts_at: DateTime<Utc>,
    duration_minutes: i32,
    max_tanks: i32,
    max_healers: i32,
    max_dps: i32,
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
}

const EVENT_COLUMNS: &str = "id, guild_id, channel_id, message_id, creator_id, title,
    description, starts_at, duration_minutes, max_tanks, max_healers, max_dps";

async fn signups_for(db: &PgPool, event_ids: &[i64]) -> AppResult<HashMap<i64, Vec<EventSignup>>> {
    let rows = sqlx::query_as::<_, SignupRow>(
        "SELECT s.event_id, s.user_id,
                COALESCE(gm.nickname, u.display_name) AS display_name,
                s.status, s.role, s.class, s.spec, s.note, s.signed_at
         FROM event_signups s
         JOIN events e ON e.id = s.event_id
         JOIN users u ON u.id = s.user_id
         LEFT JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = s.user_id
         WHERE s.event_id = ANY($1)
         ORDER BY s.signed_at",
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
        });
    }
    Ok(map)
}

fn to_event(r: EventRow, signups: Vec<EventSignup>) -> RaidEvent {
    RaidEvent {
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
        signups,
    }
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
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut signups = signups_for(db, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let s = signups.remove(&r.id).unwrap_or_default();
            (r.message_id, to_event(r, s))
        })
        .collect())
}

async fn load_row(db: &PgPool, event_id: i64) -> AppResult<EventRow> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = $1");
    sqlx::query_as::<_, EventRow>(&sql)
        .bind(event_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

/// The event plus the caller's access to its channel.
async fn load(st: &AppState, event_id: i64, user_id: i64) -> AppResult<(EventRow, ChannelAccess)> {
    let row = load_row(&st.db, event_id).await?;
    let access = channel_access(&st.db, row.channel_id, user_id).await?;
    Ok((row, access))
}

fn can_manage(row: &EventRow, access: &ChannelAccess) -> bool {
    row.creator_id == access.ctx.user_id || access.perms & p::MANAGE_EVENTS != 0
}

/// Re-sends the event's chat message so everyone's card updates.
async fn broadcast(st: &AppState, row: &EventRow) -> AppResult<Message> {
    let message = fetch_message(&st.db, row.message_id).await?;
    st.dispatch(
        Audience::Channel {
            guild_id: row.guild_id,
            channel_id: row.channel_id,
        },
        ServerEvent::MessageUpdate(message.clone()),
    );
    Ok(message)
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

// ---- Endpoints ---------------------------------------------------------

/// Posts a new raid in a channel.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateEventRequest>,
) -> AppResult<Json<Message>> {
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
    if req.starts_at < Utc::now() - chrono::Duration::hours(1) {
        return Err(AppError::BadRequest("That time has already passed.".into()));
    }

    let guild_id = access.channel.guild_id;
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
                             starts_at, duration_minutes, max_tanks, max_healers, max_dps)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
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
    Ok(Json(message))
}

pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<Json<RaidEvent>> {
    let (row, _) = load(&st, event_id, user.id).await?;
    let mut signups = signups_for(&st.db, &[row.id]).await?;
    let s = signups.remove(&row.id).unwrap_or_default();
    Ok(Json(to_event(row, s)))
}

/// Upcoming events in an outhouse that you can see (for the raid list).
pub async fn list_upcoming(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<RaidEvent>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM events
         WHERE guild_id = $1
           AND starts_at + make_interval(mins => duration_minutes) > now()
         ORDER BY starts_at LIMIT 50"
    );
    let rows = sqlx::query_as::<_, EventRow>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    let mut visible = Vec::new();
    for r in rows {
        if channel_access(&st.db, r.channel_id, user.id).await.is_ok() {
            visible.push(r);
        }
    }
    let ids: Vec<i64> = visible.iter().map(|r| r.id).collect();
    let mut signups = signups_for(&st.db, &ids).await?;
    Ok(Json(
        visible
            .into_iter()
            .map(|r| {
                let s = signups.remove(&r.id).unwrap_or_default();
                to_event(r, s)
            })
            .collect(),
    ))
}

pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<UpdateEventRequest>,
) -> AppResult<Json<Message>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&row, &access) {
        return Err(AppError::Forbidden);
    }
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
    sqlx::query(
        "UPDATE events SET title = $2, description = $3, starts_at = $4,
                duration_minutes = $5, max_tanks = $6, max_healers = $7, max_dps = $8,
                reminded = reminded AND NOT $9
         WHERE id = $1",
    )
    .bind(event_id)
    .bind(&title)
    .bind(&description)
    .bind(starts_at)
    .bind(check_duration(req.duration_minutes)?.unwrap_or(row.duration_minutes))
    .bind(check_slots(req.max_tanks)?.unwrap_or(row.max_tanks))
    .bind(check_slots(req.max_healers)?.unwrap_or(row.max_healers))
    .bind(check_slots(req.max_dps)?.unwrap_or(row.max_dps))
    .bind(moved)
    .execute(&st.db)
    .await?;
    Ok(Json(broadcast(&st, &row).await?))
}

/// Deletes the event and its chat message.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
) -> AppResult<StatusCode> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if !can_manage(&row, &access) {
        return Err(AppError::Forbidden);
    }
    sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(row.message_id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Channel {
            guild_id: row.guild_id,
            channel_id: row.channel_id,
        },
        ServerEvent::MessageDelete {
            id: row.message_id,
            channel_id: row.channel_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Signs up (or changes your sign-up). Organisers can pass `user_id` to
/// sign up or move someone else, e.g. to the bench.
pub async fn signup(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(event_id): Path<i64>,
    Json(req): Json<SignupRequest>,
) -> AppResult<Json<Message>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    let target = req.user_id.unwrap_or(user.id);
    if target != user.id {
        if !can_manage(&row, &access) {
            return Err(AppError::Forbidden);
        }
        // They must be in the outhouse.
        member_ctx(&st.db, row.guild_id, target).await?;
    } else if access.ctx.timed_out {
        return Err(AppError::Forbidden);
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
    sqlx::query(
        "INSERT INTO event_signups (event_id, user_id, status, role, class, spec, note)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (event_id, user_id) DO UPDATE
             SET status = EXCLUDED.status, role = EXCLUDED.role, class = EXCLUDED.class,
                 spec = EXCLUDED.spec, note = EXCLUDED.note,
                 -- Changing your mind keeps your place unless you were out.
                 signed_at = CASE WHEN event_signups.status = 'no' AND EXCLUDED.status <> 'no'
                                  THEN now() ELSE event_signups.signed_at END",
    )
    .bind(event_id)
    .bind(target)
    .bind(&req.status)
    .bind(&req.role)
    .bind(clean_text(&req.class, 32))
    .bind(clean_text(&req.spec, 32))
    .bind(clean_text(&req.note, 100))
    .execute(&st.db)
    .await?;
    Ok(Json(broadcast(&st, &row).await?))
}

/// Removes a sign-up (yours, or anyone's if you organise the event).
pub async fn withdraw(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((event_id, target)): Path<(i64, i64)>,
) -> AppResult<Json<Message>> {
    let (row, access) = load(&st, event_id, user.id).await?;
    if target != user.id && !can_manage(&row, &access) {
        return Err(AppError::Forbidden);
    }
    sqlx::query("DELETE FROM event_signups WHERE event_id = $1 AND user_id = $2")
        .bind(event_id)
        .bind(target)
        .execute(&st.db)
        .await?;
    Ok(Json(broadcast(&st, &row).await?))
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

/// Every minute: post a reminder for raids starting within 15 minutes.
pub fn spawn_reminders(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
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
