//! Scheduled messages: write now, send later. A background job posts them
//! when they're due, checking the person can still post there at that
//! moment; if not (lost permission, channel deleted, blocked…) the message
//! is kept as "couldn't send" and they're told.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{Message, ScheduledMessage};
use outhouse_common::permissions as p;
use serde::Deserialize;
use serde_json::json;

use crate::api::messages::{create_message_ex, thread_in_channel, MessageExtras};
use crate::auth::{fetch_user, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::channel_access;
use crate::state::{AppState, Audience};

/// Most messages one person can have waiting.
pub const MAX_PENDING: i64 = 50;
const MAX_CHARS: usize = 4000;
/// How far ahead you can schedule.
const MAX_DAYS_AHEAD: i64 = 366;

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    user_id: i64,
    channel_id: i64,
    guild_id: i64,
    thread_id: Option<i64>,
    content: String,
    send_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    status: String,
    error: Option<String>,
}

const COLUMNS: &str =
    "id, user_id, channel_id, guild_id, thread_id, content, send_at, created_at, status, error";

impl From<Row> for ScheduledMessage {
    fn from(r: Row) -> Self {
        ScheduledMessage {
            id: r.id,
            channel_id: r.channel_id,
            guild_id: r.guild_id,
            thread_id: r.thread_id,
            content: r.content,
            send_at: r.send_at,
            created_at: r.created_at,
            status: if r.status == "failed" {
                "failed".into()
            } else {
                "pending".into()
            },
            error: r.error,
        }
    }
}

fn check_content(raw: &str) -> AppResult<String> {
    let content = raw.trim().to_string();
    if content.is_empty() {
        return Err(AppError::BadRequest("Write something to send.".into()));
    }
    if content.chars().count() > MAX_CHARS {
        return Err(AppError::BadRequest(format!(
            "Messages can be up to {MAX_CHARS} characters."
        )));
    }
    Ok(content)
}

fn check_time(at: DateTime<Utc>) -> AppResult<()> {
    let now = Utc::now();
    if at < now + chrono::Duration::seconds(5) {
        return Err(AppError::BadRequest("Pick a time in the future.".into()));
    }
    if at > now + chrono::Duration::days(MAX_DAYS_AHEAD) {
        return Err(AppError::BadRequest(
            "You can schedule up to a year ahead.".into(),
        ));
    }
    Ok(())
}

async fn load(st: &AppState, id: i64, user_id: i64) -> AppResult<Row> {
    let sql = format!("SELECT {COLUMNS} FROM scheduled_messages WHERE id = $1 AND user_id = $2");
    sqlx::query_as::<_, Row>(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// `GET /api/users/@me/scheduled`: everything you have waiting (and any
/// that couldn't be sent), soonest first.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<ScheduledMessage>>> {
    let sql =
        format!("SELECT {COLUMNS} FROM scheduled_messages WHERE user_id = $1 ORDER BY send_at, id");
    let rows = sqlx::query_as::<_, Row>(&sql)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(ScheduledMessage::from).collect()))
}

#[derive(Deserialize)]
pub struct CreateScheduled {
    content: String,
    send_at: DateTime<Utc>,
    #[serde(with = "outhouse_common::id::option_as_string", default)]
    thread_id: Option<i64>,
}

/// `POST /api/channels/{id}/scheduled` `{"content", "send_at", "thread_id"?}`
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateScheduled>,
) -> AppResult<Json<ScheduledMessage>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let content = check_content(&req.content)?;
    check_time(req.send_at)?;
    if let Some(thread_id) = req.thread_id {
        thread_in_channel(&st.db, channel_id, thread_id).await?;
    }
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scheduled_messages WHERE user_id = $1 AND status <> 'failed'",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if waiting >= MAX_PENDING {
        return Err(AppError::BadRequest(format!(
            "You already have {MAX_PENDING} messages waiting to send. Send or cancel some first."
        )));
    }
    let sql = format!(
        "INSERT INTO scheduled_messages
             (id, user_id, channel_id, guild_id, thread_id, content, send_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING {COLUMNS}"
    );
    let row = sqlx::query_as::<_, Row>(&sql)
        .bind(st.ids.next_id())
        .bind(user.id)
        .bind(channel_id)
        .bind(access.channel.guild_id)
        .bind(req.thread_id)
        .bind(&content)
        .bind(req.send_at)
        .fetch_one(&st.db)
        .await?;
    let out = ScheduledMessage::from(row);
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ScheduledUpdate(out.clone()),
    );
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct UpdateScheduled {
    content: Option<String>,
    send_at: Option<DateTime<Utc>>,
}

/// `PATCH /api/scheduled/{id}` `{"content"?, "send_at"?}`. A message that
/// couldn't be sent can be given a new time to try again.
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateScheduled>,
) -> AppResult<Json<ScheduledMessage>> {
    let row = load(&st, id, user.id).await?;
    if row.status == "sending" {
        return Err(AppError::Conflict("That one's sending right now.".into()));
    }
    let content = req.content.as_deref().map(check_content).transpose()?;
    if let Some(at) = req.send_at {
        check_time(at)?;
    }
    let retry = row.status == "failed" && req.send_at.is_some();
    let sql = format!(
        "UPDATE scheduled_messages
         SET content = COALESCE($3, content), send_at = COALESCE($4, send_at),
             status = CASE WHEN $5 THEN 'pending' ELSE status END,
             error = CASE WHEN $5 THEN NULL ELSE error END
         WHERE id = $1 AND user_id = $2 AND status <> 'sending'
         RETURNING {COLUMNS}"
    );
    let row = sqlx::query_as::<_, Row>(&sql)
        .bind(id)
        .bind(user.id)
        .bind(&content)
        .bind(req.send_at)
        .bind(retry)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| AppError::Conflict("That one's sending right now.".into()))?;
    let out = ScheduledMessage::from(row);
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ScheduledUpdate(out.clone()),
    );
    Ok(Json(out))
}

/// `DELETE /api/scheduled/{id}`: cancel it.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let channel_id: Option<i64> = sqlx::query_scalar(
        "DELETE FROM scheduled_messages WHERE id = $1 AND user_id = $2 AND status <> 'sending'
         RETURNING channel_id",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let channel_id = channel_id.ok_or(AppError::NotFound)?;
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::ScheduledDelete { id, channel_id },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/scheduled/{id}/send`: send it now.
pub async fn send_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Message>> {
    let sql = format!(
        "UPDATE scheduled_messages SET status = 'sending'
         WHERE id = $1 AND user_id = $2 AND status <> 'sending'
         RETURNING {COLUMNS}"
    );
    let row = sqlx::query_as::<_, Row>(&sql)
        .bind(id)
        .bind(user.id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    match post(&st, &row).await {
        Ok(msg) => {
            finish(&st, &row).await?;
            Ok(Json(msg))
        }
        Err(e) => {
            // Back to how it was, so they can fix it or cancel it.
            sqlx::query("UPDATE scheduled_messages SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(if row.status == "failed" {
                    "failed"
                } else {
                    "pending"
                })
                .execute(&st.db)
                .await?;
            Err(e)
        }
    }
}

/// Posts a scheduled message as its author, with their permissions now.
async fn post(st: &AppState, row: &Row) -> AppResult<Message> {
    let user = fetch_user(&st.db, row.user_id).await?;
    let access = channel_access(&st.db, row.channel_id, row.user_id).await?;
    access.require(p::SEND_MESSAGES)?;
    create_message_ex(
        st,
        &access,
        &user,
        row.content.clone(),
        None,
        row.thread_id,
        Vec::new(),
        MessageExtras {
            skip_slowmode: true,
            ..Default::default()
        },
    )
    .await
}

async fn finish(st: &AppState, row: &Row) -> AppResult<()> {
    sqlx::query("DELETE FROM scheduled_messages WHERE id = $1")
        .bind(row.id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::User(row.user_id),
        ServerEvent::ScheduledDelete {
            id: row.id,
            channel_id: row.channel_id,
        },
    );
    Ok(())
}

/// Why a scheduled message couldn't go, in plain words.
fn reason(e: &AppError) -> String {
    match e {
        AppError::NotFound => "The channel is gone, or you can't see it any more.".into(),
        AppError::Forbidden => "You can't post there any more.".into(),
        AppError::AgeRestricted => {
            "The channel is age-restricted and you haven't said you're 18+.".into()
        }
        AppError::BadRequest(m) | AppError::Refused(m) | AppError::Conflict(m) => m.clone(),
        _ => "Something went wrong on the server.".into(),
    }
}

async fn fail(st: &AppState, row: &Row, why: String) -> AppResult<()> {
    let sql = format!(
        "UPDATE scheduled_messages SET status = 'failed', error = $2 WHERE id = $1
         RETURNING {COLUMNS}"
    );
    let Some(updated) = sqlx::query_as::<_, Row>(&sql)
        .bind(row.id)
        .bind(&why)
        .fetch_optional(&st.db)
        .await?
    else {
        return Ok(());
    };
    st.dispatch(
        Audience::User(row.user_id),
        ServerEvent::ScheduledUpdate(updated.into()),
    );
    if st.vapid.is_some() && st.is_away(row.user_id) {
        let preview: String = crate::timetag::plain(&row.content)
            .chars()
            .take(80)
            .collect();
        let payload = json!({
            "title": "Your scheduled message wasn't sent",
            "body": format!("{why} It said: {preview}"),
            "kind": "scheduled",
            "guild_id": row.guild_id.to_string(),
            "channel_id": row.channel_id.to_string(),
            "tag": format!("sched-{}", row.id),
            "url": if row.guild_id == 0 {
                "/app?guild=0".to_string()
            } else {
                format!("/app?guild={}&channel={}", row.guild_id, row.channel_id)
            },
        });
        if let Err(e) = crate::push::send_to_user(st, row.user_id, &payload).await {
            tracing::warn!(error = %e, "scheduled message push failed");
        }
    }
    Ok(())
}

async fn send_due(st: &AppState) -> AppResult<()> {
    let sql = format!(
        "UPDATE scheduled_messages SET status = 'sending'
         WHERE id IN (SELECT id FROM scheduled_messages
                      WHERE status = 'pending' AND send_at <= now()
                      ORDER BY send_at LIMIT 50
                      FOR UPDATE SKIP LOCKED)
         RETURNING {COLUMNS}"
    );
    let due = sqlx::query_as::<_, Row>(&sql).fetch_all(&st.db).await?;
    for row in due {
        match post(st, &row).await {
            Ok(_) => finish(st, &row).await?,
            Err(e) => {
                if matches!(e, AppError::Db(_) | AppError::Other(_)) {
                    tracing::warn!(error = %e, "scheduled message failed");
                }
                fail(st, &row, reason(&e)).await?;
            }
        }
    }
    Ok(())
}

/// Every 10 seconds: send scheduled messages that are due.
pub fn spawn_sender(st: AppState) {
    tokio::spawn(async move {
        // Anything a restart interrupted goes back in the queue.
        if let Err(e) =
            sqlx::query("UPDATE scheduled_messages SET status = 'pending' WHERE status = 'sending'")
                .execute(&st.db)
                .await
        {
            tracing::warn!(error = %e, "resetting scheduled messages failed");
        }
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        loop {
            tick.tick().await;
            if let Err(e) = send_due(&st).await {
                tracing::warn!(error = %e, "sending scheduled messages failed");
            }
        }
    });
}
