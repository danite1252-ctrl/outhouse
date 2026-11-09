//! Forwarding a message to other channels and DMs: the new message carries
//! a snapshot of the original (text, link cards, sticker) and points at the
//! same stored files, so nothing is uploaded again.

use axum::extract::{Path, State};
use axum::Json;
use outhouse_common::models::{ForwardInfo, Message, DM_GUILD};
use outhouse_common::permissions as p;
use serde::Deserialize;

use crate::api::messages::{create_message_ex, fetch_message, MessageExtras, ReusedFile};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, is_age_gated};
use crate::state::AppState;

/// Most places one message can be forwarded to at once.
pub const MAX_DESTINATIONS: usize = 5;
const MAX_NOTE_CHARS: usize = 2000;

#[derive(Deserialize)]
pub struct ForwardRequest {
    #[serde(with = "outhouse_common::id::vec_as_string")]
    destinations: Vec<i64>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(sqlx::FromRow)]
struct FileRow {
    storage_key: String,
    filename: String,
    content_type: String,
    size: i64,
    voice: bool,
    duration_ms: Option<i32>,
    waveform: Option<String>,
}

/// Where a message was: "#channel" and its outhouse, or the DM's name.
async fn place(st: &AppState, channel_id: i64) -> AppResult<(String, Option<String>)> {
    let (name, kind, guild): (String, i16, Option<String>) = sqlx::query_as(
        "SELECT c.name, c.kind, g.name FROM channels c
         LEFT JOIN guilds g ON g.id = c.guild_id WHERE c.id = $1",
    )
    .bind(channel_id)
    .fetch_one(&st.db)
    .await?;
    Ok(match guild {
        Some(g) => (format!("#{name}"), Some(g)),
        None if !name.is_empty() => (name, None),
        None if kind == outhouse_common::models::channel_kind::GROUP_DM => {
            ("a group DM".into(), None)
        }
        None => ("a DM".into(), None),
    })
}

/// `POST /api/channels/{id}/messages/{message}/forward`
/// `{"destinations": [channel or DM IDs], "note"?}`: forwards a message you
/// can see to up to five places you can post in. Out of an age-restricted
/// channel it can only go to other age-restricted channels.
pub async fn forward(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<ForwardRequest>,
) -> AppResult<Json<Vec<Message>>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let source = channel_access(&st.db, channel_id, user.id).await?;
    source.require(p::VIEW_CHANNEL | p::READ_HISTORY)?;
    let original = fetch_message(&st.db, message_id).await?;
    if original.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    let mut destinations = req.destinations.clone();
    destinations.sort_unstable();
    destinations.dedup();
    if destinations.is_empty() {
        return Err(AppError::BadRequest("Pick where to forward it.".into()));
    }
    if destinations.len() > MAX_DESTINATIONS {
        return Err(AppError::BadRequest(format!(
            "You can forward to up to {MAX_DESTINATIONS} places at once."
        )));
    }
    let note = req.note.as_deref().unwrap_or("").trim().to_string();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(AppError::BadRequest(format!(
            "Notes can be up to {MAX_NOTE_CHARS} characters."
        )));
    }

    // A forward of a forward passes on the original snapshot.
    let snapshot = match &original.forward {
        Some(inner) => inner.clone(),
        None => {
            if original.content.trim().is_empty()
                && original.attachments.is_empty()
                && original.embeds.is_empty()
                && original.sticker.is_none()
            {
                return Err(AppError::BadRequest(
                    "That kind of message can't be forwarded.".into(),
                ));
            }
            let (channel_name, guild_name) = place(&st, channel_id).await?;
            ForwardInfo {
                message_id: original.id,
                channel_id,
                guild_id: source.channel.guild_id,
                channel_name,
                guild_name,
                author_id: original.author_id,
                author_display_name: original.author_display_name.clone(),
                created_at: original.created_at,
                content: original.content.clone(),
                embeds: original.embeds.clone(),
                sticker: original.sticker.clone(),
            }
        }
    };
    let files = sqlx::query_as::<_, FileRow>(
        "SELECT storage_key, filename, content_type, size, voice, duration_ms, waveform
         FROM attachments WHERE message_id = $1 ORDER BY id",
    )
    .bind(message_id)
    .fetch_all(&st.db)
    .await?;

    // Check every destination before posting anywhere.
    let gated = is_age_gated(&source.channel);
    let mut targets = Vec::with_capacity(destinations.len());
    for dest in destinations {
        let access = channel_access(&st.db, dest, user.id)
            .await
            .map_err(|e| match e {
                AppError::NotFound => AppError::BadRequest("One of those places is gone.".into()),
                other => other,
            })?;
        if access.channel.kind != outhouse_common::models::channel_kind::TEXT
            && access.channel.guild_id != DM_GUILD
        {
            return Err(AppError::BadRequest(
                "Messages can only go to text channels and DMs.".into(),
            ));
        }
        let mut need = p::SEND_MESSAGES;
        if !files.is_empty() {
            need |= p::ATTACH_FILES;
        }
        if access.perms & need != need {
            return Err(AppError::Refused(format!(
                "You can't post {}there.",
                if files.is_empty() { "" } else { "files " }
            )));
        }
        if gated && !is_age_gated(&access.channel) {
            return Err(AppError::Refused(
                "Messages from an age-restricted channel can only be forwarded to other age-restricted channels."
                    .into(),
            ));
        }
        targets.push(access);
    }

    let mut sent = Vec::with_capacity(targets.len());
    for access in &targets {
        let reused = files
            .iter()
            .map(|f| ReusedFile {
                storage_key: f.storage_key.clone(),
                filename: f.filename.clone(),
                content_type: f.content_type.clone(),
                size: f.size,
                voice: f.voice,
                duration_ms: f.duration_ms,
                waveform: f.waveform.clone(),
            })
            .collect();
        let msg = create_message_ex(
            &st,
            access,
            &user,
            note.clone(),
            None,
            None,
            Vec::new(),
            MessageExtras {
                forward: Some(snapshot.clone()),
                reused_files: reused,
                ..Default::default()
            },
        )
        .await?;
        sent.push(msg);
    }
    Ok(Json(sent))
}
