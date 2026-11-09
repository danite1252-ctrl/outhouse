//! Threads: side conversations off a channel, usually started from a
//! message. Replies are ordinary messages in the parent channel with
//! `thread_id` set, so everything that works on messages works here too.
//!
//! Threads hide themselves (archive) after a while without activity: 1 hour,
//! 24 hours, 3 days (the default) or a week, like Discord. Archived threads
//! leave the active list; posting in one, or "Unarchive", brings it back.
//! Locked threads only open up for moderators.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, AckRequest, CreateThreadRequest, ThreadInfo, THREAD_ARCHIVE_CHOICES,
};
use outhouse_common::permissions as p;
use serde::Deserialize;

use super::messages::{fetch_message, THREAD_COLUMNS, THREAD_SELECT};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, channel_access_meta, channel_audience, ChannelAccess};
use crate::state::AppState;

fn check_archive_duration(mins: i32) -> AppResult<i32> {
    if THREAD_ARCHIVE_CHOICES.contains(&mins) {
        Ok(mins)
    } else {
        Err(AppError::BadRequest(
            "Hide after 60 minutes, 1440 (24 hours), 4320 (3 days) or 10080 (1 week).".into(),
        ))
    }
}

const MAX_NAME_CHARS: usize = 100;

fn clean_name(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect()
}

/// A default thread name from the first words of its message.
fn name_from(content: &str) -> String {
    let words: Vec<&str> = content
        .split_whitespace()
        .filter(|w| !w.starts_with('<') && !w.starts_with("http"))
        .collect();
    let mut name: String = words.join(" ").chars().take(40).collect();
    if name.chars().count() == 40 {
        name.push('…');
    }
    if name.is_empty() {
        "Thread".into()
    } else {
        name
    }
}

async fn load(
    st: &AppState,
    thread_id: i64,
    user_id: i64,
) -> AppResult<(ThreadInfo, ChannelAccess)> {
    let sql = format!("{THREAD_SELECT} WHERE id = $1");
    let thread = sqlx::query_as::<_, ThreadInfo>(&sql)
        .bind(thread_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let access = channel_access(&st.db, thread.channel_id, user_id).await?;
    Ok((thread, access))
}

/// The starter message shows "N replies", so send it again when that changes.
async fn refresh_starter(st: &AppState, access: &ChannelAccess, message_id: Option<i64>) {
    if let Some(id) = message_id {
        if let Ok(msg) = fetch_message(&st.db, id).await {
            st.dispatch(
                channel_audience(&access.channel),
                ServerEvent::MessageUpdate(msg),
            );
        }
    }
}

/// `GET /api/channels/{id}/threads`: threads in a channel, busiest first.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<Vec<ThreadInfo>>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::READ_HISTORY)?;
    let sql = format!(
        "{THREAD_SELECT} WHERE channel_id = $1
         ORDER BY (archived_at IS NOT NULL), COALESCE(last_message_id, id) DESC LIMIT 100"
    );
    let threads = sqlx::query_as::<_, ThreadInfo>(&sql)
        .bind(channel_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(threads))
}

/// `POST /api/channels/{id}/threads` `{"name", "message_id"?}`. Starting a
/// thread on a message that already has one just returns that thread.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateThreadRequest>,
) -> AppResult<Json<ThreadInfo>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    if access.channel.kind == channel_kind::VOICE {
        return Err(AppError::BadRequest(
            "Threads live in text channels.".into(),
        ));
    }
    let mut name = clean_name(&req.name);
    if let Some(message_id) = req.message_id {
        let msg = fetch_message(&st.db, message_id).await?;
        if msg.channel_id != channel_id {
            return Err(AppError::NotFound);
        }
        if msg.thread_id.is_some() {
            return Err(AppError::BadRequest(
                "You can't start a thread inside a thread.".into(),
            ));
        }
        if let Some(existing) = msg.thread {
            return Ok(Json(existing));
        }
        if name.is_empty() {
            name = name_from(&msg.content);
        }
    }
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the thread a name.".into()));
    }
    let archive_mins = match req.auto_archive_duration {
        Some(m) => check_archive_duration(m)?,
        None => {
            sqlx::query_scalar::<_, i32>("SELECT thread_archive_mins FROM channels WHERE id = $1")
                .bind(channel_id)
                .fetch_one(&st.db)
                .await?
        }
    };

    let sql = format!(
        "INSERT INTO threads (id, channel_id, starter_message_id, name, creator_id, auto_archive_mins)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (starter_message_id) DO NOTHING
         RETURNING {THREAD_COLUMNS}"
    );
    let created = sqlx::query_as::<_, ThreadInfo>(&sql)
        .bind(st.ids.next_id())
        .bind(channel_id)
        .bind(req.message_id)
        .bind(&name)
        .bind(user.id)
        .bind(archive_mins)
        .fetch_optional(&st.db)
        .await?;
    let thread = match created {
        Some(t) => t,
        // Someone else started one on the same message a moment ago.
        None => {
            let sql = format!("{THREAD_SELECT} WHERE starter_message_id = $1");
            sqlx::query_as::<_, ThreadInfo>(&sql)
                .bind(req.message_id)
                .fetch_one(&st.db)
                .await?
        }
    };
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::ThreadUpdate(thread.clone()),
    );
    refresh_starter(&st, &access, thread.starter_message_id).await;
    Ok(Json(thread))
}

/// `GET /api/threads/{id}`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(thread_id): Path<i64>,
) -> AppResult<Json<ThreadInfo>> {
    let (thread, access) = load(&st, thread_id, user.id).await?;
    access.require(p::READ_HISTORY)?;
    Ok(Json(thread))
}

#[derive(Deserialize)]
pub struct UpdateThread {
    name: Option<String>,
    /// false = unarchive (anyone who can post; moderators for locked
    /// threads), true = archive now (whoever started it, or moderators).
    archived: Option<bool>,
    /// Moderators only.
    locked: Option<bool>,
    /// 60, 1440, 4320 or 10080 minutes.
    auto_archive_duration: Option<i32>,
}

/// `PATCH /api/threads/{id}` `{"name"?, "archived"?, "locked"?,
/// "auto_archive_duration"?}`.
pub async fn rename(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(thread_id): Path<i64>,
    Json(req): Json<UpdateThread>,
) -> AppResult<Json<ThreadInfo>> {
    let (thread, access) = load(&st, thread_id, user.id).await?;
    let is_mod = access.perms & p::MANAGE_MESSAGES != 0;
    let owns = thread.creator_id == Some(user.id) && access.perms & p::SEND_MESSAGES != 0;
    let name = match &req.name {
        None => None,
        Some(raw) => {
            if !owns && !is_mod {
                return Err(AppError::Forbidden);
            }
            let name = clean_name(raw);
            if name.is_empty() {
                return Err(AppError::BadRequest("Give the thread a name.".into()));
            }
            Some(name)
        }
    };
    match req.archived {
        Some(false) => {
            access.require(p::SEND_MESSAGES)?;
            let locked = req.locked.unwrap_or(thread.locked);
            if locked && !is_mod {
                return Err(AppError::BadRequest(
                    "This thread is locked. Only moderators can bring it back.".into(),
                ));
            }
        }
        Some(true) if !owns && !is_mod => return Err(AppError::Forbidden),
        _ => {}
    }
    if req.locked.is_some() && !is_mod {
        return Err(AppError::Forbidden);
    }
    let duration = match req.auto_archive_duration {
        None => None,
        Some(m) => {
            if !owns && !is_mod {
                return Err(AppError::Forbidden);
            }
            Some(check_archive_duration(m)?)
        }
    };
    let sql = format!(
        "UPDATE threads SET name = COALESCE($2, name),
                archived_at = CASE WHEN $3::BOOLEAN IS NULL THEN archived_at
                                   WHEN $3 THEN COALESCE(archived_at, now())
                                   ELSE NULL END,
                active_at = CASE WHEN $3::BOOLEAN = FALSE THEN now() ELSE active_at END,
                locked = COALESCE($4, locked),
                auto_archive_mins = COALESCE($5, auto_archive_mins)
         WHERE id = $1
         RETURNING {THREAD_COLUMNS}"
    );
    let thread = sqlx::query_as::<_, ThreadInfo>(&sql)
        .bind(thread_id)
        .bind(&name)
        .bind(req.archived)
        .bind(req.locked)
        .bind(duration)
        .fetch_one(&st.db)
        .await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::ThreadUpdate(thread.clone()),
    );
    refresh_starter(&st, &access, thread.starter_message_id).await;
    Ok(Json(thread))
}

#[derive(serde::Serialize, Deserialize)]
pub struct ThreadDefaults {
    /// New threads hide after this many minutes without activity.
    auto_archive_duration: i32,
}

/// `GET /api/channels/{id}/thread-defaults`
pub async fn get_defaults(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<ThreadDefaults>> {
    channel_access_meta(&st.db, channel_id, user.id).await?;
    let mins: i32 = sqlx::query_scalar("SELECT thread_archive_mins FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_one(&st.db)
        .await?;
    Ok(Json(ThreadDefaults {
        auto_archive_duration: mins,
    }))
}

/// `PUT /api/channels/{id}/thread-defaults` `{"auto_archive_duration"}`
/// (needs Manage Channels): "Hide after inactivity" for new threads.
pub async fn set_defaults(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<ThreadDefaults>,
) -> AppResult<Json<ThreadDefaults>> {
    let access = channel_access_meta(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_CHANNELS)?;
    if access.channel.kind != channel_kind::TEXT {
        return Err(AppError::BadRequest(
            "Threads live in text channels.".into(),
        ));
    }
    let mins = check_archive_duration(req.auto_archive_duration)?;
    sqlx::query("UPDATE channels SET thread_archive_mins = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(mins)
        .execute(&st.db)
        .await?;
    Ok(Json(ThreadDefaults {
        auto_archive_duration: mins,
    }))
}

/// Archives threads that have gone quiet, every 15 seconds.
pub fn spawn_archiver(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            tick.tick().await;
            if let Err(e) = archive_quiet(&st).await {
                tracing::warn!(error = %e, "archiving quiet threads failed");
            }
        }
    });
}

async fn archive_quiet(st: &AppState) -> AppResult<()> {
    let sql = format!(
        "UPDATE threads SET archived_at = now()
         WHERE archived_at IS NULL
           AND active_at < now() - make_interval(mins => auto_archive_mins)
         RETURNING {THREAD_COLUMNS}"
    );
    let threads = sqlx::query_as::<_, ThreadInfo>(&sql)
        .fetch_all(&st.db)
        .await?;
    for thread in threads {
        let channel = crate::perms::fetch_channel(&st.db, thread.channel_id).await?;
        let starter = thread.starter_message_id;
        st.dispatch(
            channel_audience(&channel),
            ServerEvent::ThreadUpdate(thread),
        );
        if let Some(id) = starter {
            if let Ok(msg) = fetch_message(&st.db, id).await {
                st.dispatch(channel_audience(&channel), ServerEvent::MessageUpdate(msg));
            }
        }
    }
    Ok(())
}

/// `DELETE /api/threads/{id}`: deletes the thread and its replies (needs
/// Manage Messages).
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(thread_id): Path<i64>,
) -> AppResult<StatusCode> {
    let (thread, access) = load(&st, thread_id, user.id).await?;
    access.require(p::MANAGE_MESSAGES)?;
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT a.storage_key FROM attachments a
         JOIN messages m ON m.id = a.message_id WHERE m.thread_id = $1",
    )
    .bind(thread_id)
    .fetch_all(&st.db)
    .await?;
    sqlx::query("DELETE FROM threads WHERE id = $1")
        .bind(thread_id)
        .execute(&st.db)
        .await?;
    for key in keys {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(key)).await;
    }
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::ThreadDelete {
            id: thread_id,
            channel_id: thread.channel_id,
        },
    );
    refresh_starter(&st, &access, thread.starter_message_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/threads/{id}/ack` `{"message_id"}`: you've read the thread up
/// to that reply (opening it does this). Your other devices hear about it.
pub async fn ack(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(thread_id): Path<i64>,
    Json(req): Json<AckRequest>,
) -> AppResult<StatusCode> {
    let (thread, access) = load(&st, thread_id, user.id).await?;
    // Never past the thread's newest reply.
    let upto = thread
        .last_message_id
        .map_or(req.message_id, |l| req.message_id.min(l));
    let last_read_id: i64 = sqlx::query_scalar(
        "INSERT INTO thread_reads (user_id, thread_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, thread_id)
         DO UPDATE SET last_read_id = GREATEST(thread_reads.last_read_id, EXCLUDED.last_read_id)
         RETURNING last_read_id",
    )
    .bind(user.id)
    .bind(thread_id)
    .bind(upto)
    .fetch_one(&st.db)
    .await?;
    st.dispatch(
        crate::state::Audience::User(user.id),
        ServerEvent::ThreadAck {
            guild_id: access.channel.guild_id,
            channel_id: thread.channel_id,
            thread_id,
            last_read_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}
