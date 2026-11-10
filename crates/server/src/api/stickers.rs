//! Stickers: pictures (PNG, APNG, GIF or WebP up to 512 KB) uploaded to an
//! outhouse and sent as a message on their own, plus a few built-in
//! Outhouse ones. Like custom emoji you can use stickers from any outhouse
//! you're in, including in DMs.

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{Message, Sticker, StickerRef};
use outhouse_common::permissions as p;
use serde::Deserialize;
use sqlx::PgPool;

use crate::api::messages::{create_message_ex, MessageExtras};
use crate::audit;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, member_ctx};
use crate::state::{AppState, Audience};

pub const MAX_STICKER_BYTES: usize = 512 * 1024;
pub const MAX_STICKERS_PER_GUILD: i64 = 60;
const MAX_NAME_CHARS: usize = 30;
const MAX_DESCRIPTION_CHARS: usize = 100;
const MAX_EMOJI_CHARS: usize = 16;
const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// The built-in Outhouse stickers: (key, name, emoji, description). Their
/// pictures live in `web/builtin-stickers/{key}.svg`.
pub const BUILTINS: [(&str, &str, &str, &str); 6] = [
    (
        "occupied",
        "Occupied",
        "🚽",
        "The outhouse door says Occupied",
    ),
    ("vacant", "Vacant", "🚪", "The outhouse door says Vacant"),
    ("gg", "GG", "🧻", "A toilet roll says GG"),
    ("rekt", "Rekt", "🪠", "A plunger says Rekt"),
    ("bzzz", "Bzzz", "🪰", "A fly buzzing about"),
    ("brb", "BRB", "🌙", "Off to the dunny, back soon"),
];

fn builtin(key: &str) -> Option<Sticker> {
    BUILTINS
        .iter()
        .find(|b| b.0 == key)
        .map(|&(key, name, emoji, description)| Sticker {
            id: format!("builtin:{key}"),
            guild_id: 0,
            name: name.into(),
            emoji: emoji.into(),
            description: description.into(),
            url: format!("/builtin-stickers/{key}.svg"),
            content_type: "image/svg+xml".into(),
        })
}

#[derive(sqlx::FromRow)]
struct StickerRow {
    id: i64,
    guild_id: i64,
    name: String,
    emoji: String,
    description: String,
    content_type: String,
}

const STICKER_COLUMNS: &str = "id, guild_id, name, emoji, description, content_type";

impl From<StickerRow> for Sticker {
    fn from(r: StickerRow) -> Self {
        Sticker {
            id: r.id.to_string(),
            url: format!("/stickers/{}", r.id),
            guild_id: r.guild_id,
            name: r.name,
            emoji: r.emoji,
            description: r.description,
            content_type: r.content_type,
        }
    }
}

/// `GET /api/guilds/{id}/stickers`
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Sticker>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let sql = format!(
        "SELECT {STICKER_COLUMNS} FROM stickers WHERE guild_id = $1 ORDER BY created_at, id"
    );
    let rows = sqlx::query_as::<_, StickerRow>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows.into_iter().map(Sticker::from).collect()))
}

/// `GET /api/users/@me/stickers`: the built-in ones, then stickers from
/// every outhouse you're in.
pub async fn list_mine(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Sticker>>> {
    let rows = sqlx::query_as::<_, StickerRow>(
        "SELECT s.id, s.guild_id, s.name, s.emoji, s.description, s.content_type
         FROM stickers s
         JOIN guild_members gm ON gm.guild_id = s.guild_id AND gm.user_id = $1
         ORDER BY gm.joined_at, s.created_at, s.id",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let mut out: Vec<Sticker> = BUILTINS.iter().filter_map(|b| builtin(b.0)).collect();
    out.extend(rows.into_iter().map(Sticker::from));
    Ok(Json(out))
}

/// Works out the picture type from its first bytes (never trusts the name).
fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png") // PNG and APNG
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn bad(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!("Upload failed: {e}"))
}

fn one_line(raw: &str, max: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

fn clean_name(raw: &str) -> AppResult<String> {
    let name = one_line(raw, MAX_NAME_CHARS + 1);
    if name.chars().count() < 2 || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::BadRequest(format!(
            "Sticker names are 2–{MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

fn clean_emoji(raw: &str) -> AppResult<String> {
    let emoji = raw.trim().to_string();
    if emoji.chars().count() > MAX_EMOJI_CHARS || emoji.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest(
            "Pick one emoji that goes with the sticker.".into(),
        ));
    }
    Ok(emoji)
}

fn clean_description(raw: &str) -> AppResult<String> {
    let text = one_line(raw, MAX_DESCRIPTION_CHARS + 1);
    if text.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(AppError::BadRequest(format!(
            "Descriptions can be up to {MAX_DESCRIPTION_CHARS} characters."
        )));
    }
    Ok(text)
}

async fn check_name_free(
    db: &PgPool,
    guild_id: i64,
    name: &str,
    except: Option<i64>,
) -> AppResult<()> {
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM stickers
                       WHERE guild_id = $1 AND lower(name) = lower($2)
                         AND ($3::BIGINT IS NULL OR id <> $3))",
    )
    .bind(guild_id)
    .bind(name)
    .bind(except)
    .fetch_one(db)
    .await?;
    if taken {
        return Err(AppError::Conflict(format!(
            "There's already a sticker called {name} here."
        )));
    }
    Ok(())
}

/// `POST /api/guilds/{id}/stickers`: multipart `name`, `emoji`,
/// `description` and `file`. Needs Manage Outhouse.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    mut form: Multipart,
) -> AppResult<Json<Sticker>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let mut name = String::new();
    let mut emoji = String::new();
    let mut description = String::new();
    let mut data: Option<axum::body::Bytes> = None;
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "name" => name = field.text().await.map_err(bad)?,
            "emoji" => emoji = field.text().await.map_err(bad)?,
            "description" => description = field.text().await.map_err(bad)?,
            "file" => data = Some(field.bytes().await.map_err(bad)?),
            _ => {}
        }
    }
    let name = clean_name(&name)?;
    let emoji = clean_emoji(&emoji)?;
    let description = clean_description(&description)?;
    let data = data.ok_or_else(|| AppError::BadRequest("Pick a picture.".into()))?;
    if data.len() > MAX_STICKER_BYTES {
        return Err(AppError::BadRequest(
            "Sticker pictures can be up to 512 KB.".into(),
        ));
    }
    let content_type = sniff(&data).ok_or_else(|| {
        AppError::BadRequest("Stickers must be a PNG, APNG, GIF or WebP picture.".into())
    })?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM stickers WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    if count >= MAX_STICKERS_PER_GUILD {
        return Err(AppError::BadRequest(format!(
            "This outhouse already has {MAX_STICKERS_PER_GUILD} stickers. Delete one first."
        )));
    }
    check_name_free(&st.db, guild_id, &name, None).await?;

    let key = auth::random_code(24, KEY_ALPHABET);
    tokio::fs::create_dir_all(&st.config.upload_dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
    tokio::fs::write(st.config.upload_dir.join(&key), &data)
        .await
        .map_err(|e| anyhow::anyhow!("saving sticker: {e}"))?;
    let id = st.ids.next_id();
    let inserted = sqlx::query(
        "INSERT INTO stickers
             (id, guild_id, name, emoji, description, storage_key, content_type, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(&name)
    .bind(&emoji)
    .bind(&description)
    .bind(&key)
    .bind(content_type)
    .bind(user.id)
    .execute(&st.db)
    .await;
    if let Err(e) = inserted {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&key)).await;
        return Err(e.into());
    }
    crate::api::detector::track_upload(
        &st,
        crate::api::detector::KIND_STICKER,
        &key,
        content_type,
        user.id,
        Some(guild_id),
    )
    .await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::StickersChanged { guild_id },
    );
    audit::Entry::new(guild_id, user.id, "sticker_create")
        .target("sticker", id, name.clone())
        .save(&st)
        .await;
    Ok(Json(Sticker {
        id: id.to_string(),
        guild_id,
        url: format!("/stickers/{id}"),
        name,
        emoji,
        description,
        content_type: content_type.into(),
    }))
}

async fn sticker_guild(db: &PgPool, sticker_id: i64) -> AppResult<(i64, String, String)> {
    sqlx::query_as::<_, (i64, String, String)>(
        "SELECT guild_id, storage_key, name FROM stickers WHERE id = $1",
    )
    .bind(sticker_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub struct UpdateSticker {
    name: Option<String>,
    emoji: Option<String>,
    description: Option<String>,
}

/// `PATCH /api/stickers/{id}` `{"name"?, "emoji"?, "description"?}`
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(sticker_id): Path<i64>,
    Json(req): Json<UpdateSticker>,
) -> AppResult<Json<Sticker>> {
    let (guild_id, _, old_name) = sticker_guild(&st.db, sticker_id).await?;
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let name = req.name.as_deref().map(clean_name).transpose()?;
    if let Some(name) = &name {
        check_name_free(&st.db, guild_id, name, Some(sticker_id)).await?;
    }
    let emoji = req.emoji.as_deref().map(clean_emoji).transpose()?;
    let description = req
        .description
        .as_deref()
        .map(clean_description)
        .transpose()?;
    let sql = format!(
        "UPDATE stickers SET name = COALESCE($2, name), emoji = COALESCE($3, emoji),
                description = COALESCE($4, description)
         WHERE id = $1 RETURNING {STICKER_COLUMNS}"
    );
    let row = sqlx::query_as::<_, StickerRow>(&sql)
        .bind(sticker_id)
        .bind(&name)
        .bind(&emoji)
        .bind(&description)
        .fetch_one(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::StickersChanged { guild_id },
    );
    let mut entry = audit::Entry::new(guild_id, user.id, "sticker_update").target(
        "sticker",
        sticker_id,
        row.name.clone(),
    );
    if row.name != old_name {
        entry = entry.details(format!("{old_name} → {}", row.name));
    }
    entry.save(&st).await;
    Ok(Json(row.into()))
}

/// `DELETE /api/stickers/{id}`. Messages that used it show "Sticker
/// deleted" from then on.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(sticker_id): Path<i64>,
) -> AppResult<StatusCode> {
    let (guild_id, key, name) = sticker_guild(&st.db, sticker_id).await?;
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    sqlx::query("DELETE FROM stickers WHERE id = $1")
        .bind(sticker_id)
        .execute(&st.db)
        .await?;
    // Kept while a safety report refers to it.
    if !crate::api::safety::file_retained(&st.db, &key).await {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&key)).await;
    }
    audit::Entry::new(guild_id, user.id, "sticker_delete")
        .target("sticker", sticker_id, name)
        .save(&st)
        .await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::StickersChanged { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /stickers/{id}`: a custom sticker's picture.
pub async fn serve(State(st): State<AppState>, Path(id): Path<String>) -> AppResult<Response> {
    let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
    let (key, name, content_type): (String, String, String) =
        sqlx::query_as("SELECT storage_key, name, content_type FROM stickers WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(AppError::NotFound)?;
    crate::files::send_public_file(&st, &key, &name, &content_type).await
}

#[derive(Deserialize)]
pub struct SendSticker {
    /// A custom sticker's ID or `builtin:<key>`.
    sticker_id: String,
    #[serde(with = "outhouse_common::id::option_as_string", default)]
    reply_to_id: Option<i64>,
    #[serde(with = "outhouse_common::id::option_as_string", default)]
    thread_id: Option<i64>,
}

/// Which sticker someone may send: a built-in one, or one from an outhouse
/// they're in.
pub(crate) async fn usable_sticker(db: &PgPool, user_id: i64, raw: &str) -> AppResult<StickerRef> {
    let unknown = || AppError::BadRequest("That sticker isn't available.".into());
    if let Some(key) = raw.strip_prefix("builtin:") {
        let s = builtin(key).ok_or_else(unknown)?;
        return Ok(StickerRef {
            id: s.id,
            name: s.name,
            url: s.url,
        });
    }
    let id: i64 = raw.trim().parse().map_err(|_| unknown())?;
    let name: Option<String> = sqlx::query_scalar(
        "SELECT s.name FROM stickers s
         JOIN guild_members gm ON gm.guild_id = s.guild_id AND gm.user_id = $2
         WHERE s.id = $1",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let name = name.ok_or_else(unknown)?;
    Ok(StickerRef {
        id: id.to_string(),
        name,
        url: format!("/stickers/{id}"),
    })
}

/// `POST /api/channels/{id}/stickers` `{"sticker_id", "reply_to_id"?,
/// "thread_id"?}`: sends a sticker as a message.
pub async fn send(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<SendSticker>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let sticker = usable_sticker(&st.db, user.id, &req.sticker_id).await?;
    let msg = create_message_ex(
        &st,
        &access,
        &user,
        String::new(),
        req.reply_to_id,
        req.thread_id,
        Vec::new(),
        MessageExtras {
            sticker: Some(sticker),
            ..Default::default()
        },
    )
    .await?;
    Ok(Json(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_pictures_and_unique_keys() {
        let mut keys: Vec<&str> = BUILTINS.iter().map(|b| b.0).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), BUILTINS.len());
        assert_eq!(builtin("gg").unwrap().id, "builtin:gg");
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"<svg onload=alert(1)>"), None);
    }

    #[test]
    fn names_and_emoji() {
        assert!(clean_name("x").is_err());
        assert_eq!(clean_name("  Big   Pull ").unwrap(), "Big Pull");
        assert!(clean_name(&"a".repeat(31)).is_err());
        assert!(clean_emoji("🔥").is_ok());
        assert!(clean_emoji("two words").is_err());
    }
}
