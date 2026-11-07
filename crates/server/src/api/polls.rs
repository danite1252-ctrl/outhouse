//! Polls: a question with 2–10 options, posted as a message. Votes update
//! live (the message is sent again) and polls with an end time close
//! themselves.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, CreatePollRequest, Message, PollVoteRequest};
use outhouse_common::permissions as p;

use super::messages::fetch_message;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, channel_audience, fetch_channel};
use crate::state::AppState;

pub const MAX_OPTIONS: usize = 10;
const MAX_QUESTION_CHARS: usize = 300;
const MAX_OPTION_CHARS: usize = 100;
/// A week.
const MAX_DURATION_MINUTES: i64 = 7 * 24 * 60;

fn clean(raw: &str, max: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// `POST /api/channels/{id}/polls`
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreatePollRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    if access.channel.kind == channel_kind::VOICE {
        return Err(AppError::BadRequest("Polls go in text channels.".into()));
    }
    let question = clean(&req.question, MAX_QUESTION_CHARS);
    if question.is_empty() {
        return Err(AppError::BadRequest("Ask a question.".into()));
    }
    let options: Vec<String> = req
        .options
        .iter()
        .map(|o| clean(o, MAX_OPTION_CHARS))
        .filter(|o| !o.is_empty())
        .collect();
    if options.len() < 2 || options.len() > MAX_OPTIONS {
        return Err(AppError::BadRequest(format!(
            "A poll needs 2 to {MAX_OPTIONS} options."
        )));
    }
    let mut seen = std::collections::HashSet::new();
    if !options.iter().all(|o| seen.insert(o.to_lowercase())) {
        return Err(AppError::BadRequest(
            "Each option has to be different.".into(),
        ));
    }
    let ends_at: Option<DateTime<Utc>> = match req.duration_minutes {
        None | Some(0) => None,
        Some(m) if (1..=MAX_DURATION_MINUTES).contains(&m) => {
            Some(Utc::now() + chrono::Duration::minutes(m))
        }
        Some(_) => {
            return Err(AppError::BadRequest(
                "Polls can run for up to a week.".into(),
            ))
        }
    };

    let id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, channel_id, author_id, content) VALUES ($1, $2, $3, '')",
    )
    .bind(id)
    .bind(channel_id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO polls (message_id, question, options, multi, ends_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&question)
    .bind(&options)
    .bind(req.multi)
    .bind(ends_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_id = EXCLUDED.last_read_id",
    )
    .bind(user.id)
    .bind(channel_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let message = fetch_message(&st.db, id).await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::MessageCreate(message.clone()),
    );
    Ok(Json(message))
}

#[derive(sqlx::FromRow)]
struct PollState {
    channel_id: i64,
    author_id: i64,
    option_count: i32,
    multi: bool,
    open: bool,
}

async fn poll_state(st: &AppState, channel_id: i64, message_id: i64) -> AppResult<PollState> {
    let poll = sqlx::query_as::<_, PollState>(
        "SELECT m.channel_id, m.author_id, cardinality(p.options) AS option_count, p.multi,
                (NOT p.closed AND (p.ends_at IS NULL OR p.ends_at > now())) AS open
         FROM polls p JOIN messages m ON m.id = p.message_id
         WHERE p.message_id = $1",
    )
    .bind(message_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if poll.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    Ok(poll)
}

/// `PUT /api/channels/{id}/polls/{message}/votes` `{"options": [0, 2]}`:
/// replaces your votes (an empty list takes them back).
pub async fn vote(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<PollVoteRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::ADD_REACTIONS)?;
    let poll = poll_state(&st, channel_id, message_id).await?;
    if !poll.open {
        return Err(AppError::BadRequest("This poll has closed.".into()));
    }
    let mut picks = req.options.clone();
    picks.sort_unstable();
    picks.dedup();
    if picks.iter().any(|&i| i < 0 || i >= poll.option_count) {
        return Err(AppError::BadRequest("That option doesn't exist.".into()));
    }
    if !poll.multi && picks.len() > 1 {
        return Err(AppError::BadRequest("Pick just one option.".into()));
    }

    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM poll_votes WHERE message_id = $1 AND user_id = $2")
        .bind(message_id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    for i in &picks {
        sqlx::query("INSERT INTO poll_votes (message_id, user_id, option_idx) VALUES ($1, $2, $3)")
            .bind(message_id)
            .bind(user.id)
            .bind(i)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let message = fetch_message(&st.db, message_id).await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::MessageUpdate(message.clone()),
    );
    Ok(Json(message))
}

/// `POST /api/channels/{id}/polls/{message}/close`: whoever asked, or moderators.
pub async fn close(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let poll = poll_state(&st, channel_id, message_id).await?;
    if poll.author_id != user.id {
        access.require(p::MANAGE_MESSAGES)?;
    }
    sqlx::query("UPDATE polls SET closed = true WHERE message_id = $1")
        .bind(message_id)
        .execute(&st.db)
        .await?;
    let message = fetch_message(&st.db, message_id).await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::MessageUpdate(message.clone()),
    );
    Ok(Json(message))
}

/// Closes polls whose time is up, every 15 seconds.
pub fn spawn_closer(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            if let Err(e) = close_due(&st).await {
                tracing::warn!(error = %e, "closing polls failed");
            }
        }
    });
}

async fn close_due(st: &AppState) -> AppResult<()> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "UPDATE polls SET closed = true
         WHERE NOT closed AND ends_at IS NOT NULL AND ends_at <= now()
         RETURNING message_id",
    )
    .fetch_all(&st.db)
    .await?;
    for id in ids {
        let message = fetch_message(&st.db, id).await?;
        let channel = fetch_channel(&st.db, message.channel_id).await?;
        st.dispatch(
            channel_audience(&channel),
            ServerEvent::MessageUpdate(message),
        );
    }
    Ok(())
}
