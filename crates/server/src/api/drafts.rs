//! Drafts: files added to a message box, uploaded straight away (before the
//! message is sent) so the image detector can check pictures first. A
//! picture it flags is then held from the moment it's sent, with no window
//! where anyone could see it; the sender's message box shows "Checking…"
//! and "Will be held for review" meanwhile.
//!
//! A draft belongs to its uploader, one channel and (optionally) one
//! thread. It's never served to anyone (the message box previews the file
//! from the uploader's own device). Sending a message with `draft_ids`
//! turns the drafts into its attachments (api::messages); removing one from
//! the message box deletes it. Unsent drafts go after an hour, except a
//! picture escalated to child safety (explicit with a face estimated
//! clearly young): that's kept as evidence by its automatic child-safety
//! report, made even though it was never sent. A flagged picture that's
//! never sent is just deleted (nobody saw it).

use std::time::Duration;

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::permissions as p;
use serde_json::{json, Value};

use super::messages::{
    check_dm_blocks, clean_filename, guess_content_type, thread_in_channel, KEY_ALPHABET,
    MAX_FILE_BYTES,
};
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::channel_access;
use crate::state::AppState;

/// Drafts one person can have waiting at once.
pub const MAX_LIVE_DRAFTS: i64 = 20;
/// How long an unsent draft is kept.
pub const DRAFT_TTL_MINS: i32 = 60;
/// Draft uploads per person per minute.
const UPLOADS_PER_WINDOW: u32 = 40;
const UPLOAD_WINDOW: Duration = Duration::from_secs(60);

/// A draft about to become a message's attachment.
pub(crate) struct DraftFile {
    pub id: i64,
    pub storage_key: String,
    pub filename: String,
    pub content_type: String,
    pub size: i64,
}

#[derive(sqlx::FromRow)]
struct DraftRow {
    id: i64,
    storage_key: String,
    filename: String,
    content_type: String,
    size: i64,
    expires_at: DateTime<Utc>,
}

fn gone() -> AppError {
    AppError::BadRequest(
        "A file you added is gone (it was sent, removed or expired). Remove it and add it again."
            .into(),
    )
}

fn draft_json(d: &DraftRow, state: &str) -> Value {
    json!({
        "draft_id": d.id.to_string(),
        "filename": d.filename,
        "content_type": d.content_type,
        "size": d.size,
        "state": state,
        "expires_at": d.expires_at,
    })
}

fn upload_error(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!("Upload failed: {e}"))
}

/// `POST /api/channels/{id}/drafts`: one file (`file`), optional
/// `thread_id`. Checked like a message upload (Attach Files in that
/// channel, 25 MB); the detector starts on it straight away.
pub async fn upload(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    mut form: Multipart,
) -> AppResult<Json<Value>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES | p::ATTACH_FILES)?;
    check_dm_blocks(&st, &access, user.id).await?;
    if crate::security::get().rate_limits
        && !crate::security::limiter().allow(
            &format!("draft:{}", user.id),
            UPLOADS_PER_WINDOW,
            UPLOAD_WINDOW,
        )
    {
        return Err(AppError::TooManyRequests(
            "You're adding files too fast. Give it a minute.".into(),
        ));
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM message_drafts WHERE user_id = $1 AND expires_at > now()",
    )
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if live >= MAX_LIVE_DRAFTS {
        return Err(AppError::TooManyRequests(format!(
            "You have {MAX_LIVE_DRAFTS} files waiting to be sent. Send or remove some first."
        )));
    }

    let mut thread_id = None;
    let mut file = None;
    while let Some(field) = form.next_field().await.map_err(upload_error)? {
        match field.name().unwrap_or_default() {
            "thread_id" => {
                let text = field.text().await.map_err(upload_error)?;
                let text = text.trim();
                if !text.is_empty() {
                    thread_id = Some(
                        text.parse::<i64>()
                            .map_err(|_| AppError::BadRequest("Bad thread_id.".into()))?,
                    );
                }
            }
            "file" | "files" => {
                if file.is_some() {
                    return Err(AppError::BadRequest("One file at a time.".into()));
                }
                let filename = clean_filename(field.file_name().unwrap_or("file"));
                let data = field.bytes().await.map_err(upload_error)?;
                if data.len() > MAX_FILE_BYTES {
                    return Err(AppError::BadRequest(format!(
                        "{filename} is too big (25 MB max)."
                    )));
                }
                file = Some((filename, data));
            }
            _ => {}
        }
    }
    let Some((filename, data)) = file else {
        return Err(AppError::BadRequest("No file.".into()));
    };
    if let Some(t) = thread_id {
        thread_in_channel(&st.db, channel_id, t)
            .await
            .map_err(|_| AppError::BadRequest("That thread is gone.".into()))?;
    }
    let content_type = guess_content_type(&filename).to_string();
    let key = auth::random_code(24, KEY_ALPHABET);
    tokio::fs::create_dir_all(&st.config.upload_dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
    tokio::fs::write(st.config.upload_dir.join(&key), &data)
        .await
        .map_err(|e| anyhow::anyhow!("saving upload: {e}"))?;
    let row = sqlx::query_as::<_, DraftRow>(
        "INSERT INTO message_drafts (id, user_id, channel_id, thread_id, storage_key, filename,
                                     content_type, size, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(mins => $9))
         RETURNING id, storage_key, filename, content_type, size, expires_at",
    )
    .bind(st.ids.next_id())
    .bind(user.id)
    .bind(channel_id)
    .bind(thread_id)
    .bind(&key)
    .bind(&filename)
    .bind(&content_type)
    .bind(data.len() as i64)
    .bind(DRAFT_TTL_MINS)
    .fetch_one(&st.db)
    .await;
    let row = match row {
        Ok(r) => r,
        Err(e) => {
            let _ = tokio::fs::remove_file(st.config.upload_dir.join(&key)).await;
            return Err(e.into());
        }
    };
    let state =
        super::detector::track_draft(&st, &access.channel, &user, &key, &content_type).await;
    Ok(Json(draft_json(&row, state)))
}

async fn own_draft(st: &AppState, user_id: i64, id: i64) -> AppResult<DraftRow> {
    sqlx::query_as::<_, DraftRow>(
        "SELECT id, storage_key, filename, content_type, size, expires_at FROM message_drafts
         WHERE id = $1 AND user_id = $2 AND expires_at > now()",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)
}

/// `GET /api/drafts/{id}`: a draft's check (yours only).
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    let d = own_draft(&st, user.id, id).await?;
    let state = super::detector::draft_state_of(&st, &d.storage_key).await;
    Ok(Json(draft_json(&d, state)))
}

/// `DELETE /api/drafts/{id}`: removed from the message box.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let key: Option<String> = sqlx::query_scalar(
        "DELETE FROM message_drafts WHERE id = $1 AND user_id = $2 RETURNING storage_key",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let key = key.ok_or(AppError::NotFound)?;
    forget_file(&st, &key).await;
    Ok(StatusCode::NO_CONTENT)
}

/// The drafts a message is being sent with: the sender's own, for this
/// channel and thread, not expired or used, and not quarantined. (They're
/// taken for good inside the message's transaction.)
pub(crate) async fn for_message(
    st: &AppState,
    user_id: i64,
    channel_id: i64,
    thread_id: Option<i64>,
    ids: &[i64],
) -> AppResult<Vec<DraftFile>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut unique = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != ids.len() {
        return Err(AppError::BadRequest(
            "The same file was added twice.".into(),
        ));
    }
    let rows = sqlx::query_as::<_, DraftRow>(
        "SELECT id, storage_key, filename, content_type, size, expires_at FROM message_drafts
         WHERE id = ANY($1) AND user_id = $2 AND channel_id = $3
           AND thread_id IS NOT DISTINCT FROM $4 AND expires_at > now()",
    )
    .bind(ids)
    .bind(user_id)
    .bind(channel_id)
    .bind(thread_id)
    .fetch_all(&st.db)
    .await?;
    if rows.len() != ids.len() {
        return Err(gone());
    }
    let mut out = Vec::with_capacity(ids.len());
    // In the order they were given (the message box's order).
    for id in ids {
        let d = rows.iter().find(|d| d.id == *id).ok_or_else(gone)?;
        if super::safety::is_quarantined(&st.db, &d.storage_key).await
            || escalated(st, &d.storage_key).await
        {
            return Err(AppError::Refused(format!(
                "{} can't be sent. Remove it from your message.",
                d.filename
            )));
        }
        out.push(DraftFile {
            id: d.id,
            storage_key: d.storage_key.clone(),
            filename: d.filename.clone(),
            content_type: d.content_type.clone(),
            size: d.size,
        });
    }
    Ok(out)
}

async fn escalated(st: &AppState, key: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM file_scans WHERE storage_key = $1
                       AND age_escalated_at IS NOT NULL AND age_cleared_at IS NULL)",
    )
    .bind(key)
    .fetch_one(&st.db)
    .await
    .unwrap_or(true)
}

/// Takes the drafts for a message, inside its transaction: each must still
/// be there (two messages can't both use one).
pub(crate) async fn take(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: i64,
    drafts: &[DraftFile],
) -> AppResult<()> {
    if drafts.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = drafts.iter().map(|d| d.id).collect();
    let taken = sqlx::query("DELETE FROM message_drafts WHERE id = ANY($1) AND user_id = $2")
        .bind(&ids)
        .bind(user_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if taken as usize != ids.len() {
        return Err(gone());
    }
    Ok(())
}

/// A draft's file once the draft is gone (removed or expired): deleted,
/// unless it's still being checked (it's tidied up once the check is done,
/// so a picture can't dodge the age rule by being removed quickly) or it's
/// evidence for a report.
async fn forget_file(st: &AppState, key: &str) -> bool {
    let scan: Option<String> =
        sqlx::query_scalar("SELECT status FROM file_scans WHERE storage_key = $1")
            .bind(key)
            .fetch_optional(&st.db)
            .await
            .unwrap_or(Some("pending".into()));
    if scan.as_deref() == Some("pending") || super::safety::file_retained(&st.db, key).await {
        return false;
    }
    let _ = sqlx::query(
        "DELETE FROM file_scans WHERE storage_key = $1 AND kind = 'draft' AND status <> 'pending'",
    )
    .bind(key)
    .execute(&st.db)
    .await;
    super::messages::remove_unused_files(st, vec![key.to_string()]).await;
    true
}

/// Expired drafts, and files of removed drafts whose check has finished.
/// Returns how many files were deleted.
pub(crate) async fn sweep(st: &AppState) -> AppResult<usize> {
    let expired: Vec<String> = sqlx::query_scalar(
        "DELETE FROM message_drafts WHERE expires_at <= now() RETURNING storage_key",
    )
    .fetch_all(&st.db)
    .await?;
    let leftover: Vec<String> = sqlx::query_scalar(
        "SELECT s.storage_key FROM file_scans s
         WHERE s.kind = 'draft' AND s.status <> 'pending'
           AND NOT EXISTS(SELECT 1 FROM message_drafts d WHERE d.storage_key = s.storage_key)
         LIMIT 500",
    )
    .fetch_all(&st.db)
    .await?;
    let mut removed = 0;
    for key in expired.iter().chain(leftover.iter()) {
        if forget_file(st, key).await {
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn spawn_sweeper(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5 * 60));
        loop {
            tick.tick().await;
            match sweep(&st).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "removed unsent draft files"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "couldn't tidy up drafts"),
            }
        }
    });
}

/// `POST /api/drafts/sweep`: the hourly tidy-up, now. Only with
/// `OUTHOUSE_TEST_HOOKS=1` (the smoke test backdates drafts in SQL).
pub async fn sweep_now(
    State(st): State<AppState>,
    AuthUser(_user): AuthUser,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    let removed = sweep(&st).await?;
    Ok(Json(json!({ "removed": removed })))
}
