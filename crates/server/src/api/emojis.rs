//! Custom emoji: pictures uploaded to an outhouse, used as `:name:` in
//! messages and as reactions. Stored in messages as `<:name:id>` so a
//! rename or a clash between outhouses never changes old messages.

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::CustomEmoji;
use outhouse_common::permissions as p;
use serde::Deserialize;
use sqlx::PgPool;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::{AppState, Audience};

pub const MAX_EMOJI_BYTES: usize = 256 * 1024;
const MAX_EMOJIS_PER_GUILD: i64 = 100;
const KEY_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

// ---- Text handling -------------------------------------------------------------

fn name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
}

/// Emoji names: 2–32 of a–z, 0–9 and _, with at least one letter or _
/// (so times like 12:30:45 are left alone).
pub fn valid_name(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name.bytes().all(name_byte)
        && name.bytes().any(|b| !b.is_ascii_digit())
}

/// Length of a `<:name:id>` token at the start of `s`, if there is one.
fn token_len(s: &str) -> Option<usize> {
    let rest = s.strip_prefix("<:")?;
    let colon = rest.find(':')?;
    let name = &rest[..colon];
    let after = &rest[colon + 1..];
    let close = after.find('>')?;
    let digits = &after[..close];
    if !valid_name(name)
        || digits.is_empty()
        || digits.len() > 20
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(2 + colon + 1 + close + 1)
}

/// Splits `<:name:id>` into its name and ID.
pub fn parse_token(s: &str) -> Option<(&str, i64)> {
    if token_len(s)? != s.len() {
        return None;
    }
    let inner = &s[2..s.len() - 1];
    let (name, id) = inner.split_once(':')?;
    Some((name, id.parse().ok()?))
}

/// Byte ranges of `:name:` shortcodes, skipping `code` and existing tokens.
fn shortcodes(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut in_code = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'`' => {
                in_code = !in_code;
                i += 1;
            }
            b'<' if !in_code => i += token_len(&text[i..]).unwrap_or(1),
            b':' if !in_code => {
                let mut j = i + 1;
                while j < b.len() && name_byte(b[j]) {
                    j += 1;
                }
                if j < b.len() && b[j] == b':' && valid_name(&text[i + 1..j]) {
                    out.push((i, j + 1));
                    i = j + 1;
                } else {
                    i = j.max(i + 1);
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Swaps each `:name:` that `lookup` knows for `<:name:id>`.
pub fn replace_shortcodes(text: &str, lookup: impl Fn(&str) -> Option<i64>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in shortcodes(text) {
        let name = &text[start + 1..end - 1];
        if let Some(id) = lookup(name) {
            out.push_str(&text[last..start]);
            out.push_str(&format!("<:{name}:{id}>"));
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

#[derive(sqlx::FromRow)]
struct NameId {
    name: String,
    id: i64,
}

/// Turns `:name:` into custom emoji tokens. The outhouse you're posting in
/// wins; otherwise any outhouse you're in (that's how DMs get emoji).
pub async fn expand(db: &PgPool, guild_id: i64, user_id: i64, content: &str) -> AppResult<String> {
    let names: Vec<String> = shortcodes(content)
        .into_iter()
        .map(|(s, e)| content[s + 1..e - 1].to_string())
        .collect();
    if names.is_empty() {
        return Ok(content.to_string());
    }
    let found = sqlx::query_as::<_, NameId>(
        "SELECT DISTINCT ON (e.name) e.name, e.id
         FROM emojis e
         JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = $1
         WHERE e.name = ANY($2)
         ORDER BY e.name, (e.guild_id = $3) DESC, gm.joined_at",
    )
    .bind(user_id)
    .bind(&names)
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    if found.is_empty() {
        return Ok(content.to_string());
    }
    Ok(replace_shortcodes(content, |name| {
        found.iter().find(|f| f.name == name).map(|f| f.id)
    }))
}

// ---- Endpoints -------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct EmojiRow {
    id: i64,
    guild_id: i64,
    name: String,
    animated: bool,
}

impl From<EmojiRow> for CustomEmoji {
    fn from(r: EmojiRow) -> Self {
        CustomEmoji {
            url: format!("/emojis/{}", r.id),
            id: r.id,
            guild_id: r.guild_id,
            name: r.name,
            animated: r.animated,
        }
    }
}

/// `GET /api/guilds/{id}/emojis`
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<CustomEmoji>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let rows = sqlx::query_as::<_, EmojiRow>(
        "SELECT id, guild_id, name, animated FROM emojis WHERE guild_id = $1 ORDER BY name",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(CustomEmoji::from).collect()))
}

/// `GET /api/users/@me/emojis`: emoji from every outhouse you're in.
pub async fn list_mine(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<CustomEmoji>>> {
    let rows = sqlx::query_as::<_, EmojiRow>(
        "SELECT e.id, e.guild_id, e.name, e.animated
         FROM emojis e
         JOIN guild_members gm ON gm.guild_id = e.guild_id AND gm.user_id = $1
         ORDER BY gm.joined_at, e.name",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(CustomEmoji::from).collect()))
}

/// Works out the picture type from its first bytes (never trusts the name).
fn sniff(data: &[u8]) -> Option<(&'static str, bool)> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", false))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(("image/gif", true))
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        // Animated WebP has an ANIM chunk.
        let animated = data.windows(4).take(4096).any(|w| w == b"ANIM");
        Some(("image/webp", animated))
    } else {
        None
    }
}

fn bad(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!("Upload failed: {e}"))
}

async fn check_name_free(db: &PgPool, guild_id: i64, name: &str) -> AppResult<()> {
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM emojis WHERE guild_id = $1 AND name = $2)")
            .bind(guild_id)
            .bind(name)
            .fetch_one(db)
            .await?;
    if taken {
        return Err(AppError::Conflict(format!(
            "There's already an emoji called :{name}: here."
        )));
    }
    Ok(())
}

fn clean_name(raw: &str) -> AppResult<String> {
    let name = raw.trim().trim_matches(':').to_lowercase();
    if !valid_name(&name) {
        return Err(AppError::BadRequest(
            "Emoji names are 2–32 letters, numbers or _ (like raid_boss).".into(),
        ));
    }
    Ok(name)
}

/// `POST /api/guilds/{id}/emojis`: multipart `name` and `file` (PNG, GIF or
/// WebP, up to 256 KB). Needs Manage Outhouse.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    mut form: Multipart,
) -> AppResult<Json<CustomEmoji>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let mut name = String::new();
    let mut data: Option<axum::body::Bytes> = None;
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "name" => name = field.text().await.map_err(bad)?,
            "file" => data = Some(field.bytes().await.map_err(bad)?),
            _ => {}
        }
    }
    let name = clean_name(&name)?;
    let data = data.ok_or_else(|| AppError::BadRequest("Pick a picture.".into()))?;
    if data.len() > MAX_EMOJI_BYTES {
        return Err(AppError::BadRequest(
            "Emoji pictures can be up to 256 KB.".into(),
        ));
    }
    let (content_type, animated) = sniff(&data)
        .ok_or_else(|| AppError::BadRequest("Emoji must be a PNG, GIF or WebP picture.".into()))?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM emojis WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    if count >= MAX_EMOJIS_PER_GUILD {
        return Err(AppError::BadRequest(format!(
            "This outhouse already has {MAX_EMOJIS_PER_GUILD} emoji. Delete one first."
        )));
    }
    check_name_free(&st.db, guild_id, &name).await?;

    let key = auth::random_code(24, KEY_ALPHABET);
    tokio::fs::create_dir_all(&st.config.upload_dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating upload folder: {e}"))?;
    tokio::fs::write(st.config.upload_dir.join(&key), &data)
        .await
        .map_err(|e| anyhow::anyhow!("saving emoji: {e}"))?;
    let id = st.ids.next_id();
    let inserted = sqlx::query(
        "INSERT INTO emojis (id, guild_id, name, storage_key, content_type, animated, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(&name)
    .bind(&key)
    .bind(content_type)
    .bind(animated)
    .bind(user.id)
    .execute(&st.db)
    .await;
    if let Err(e) = inserted {
        let _ = tokio::fs::remove_file(st.config.upload_dir.join(&key)).await;
        return Err(e.into());
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::EmojisChanged { guild_id },
    );
    Ok(Json(CustomEmoji {
        id,
        guild_id,
        url: format!("/emojis/{id}"),
        name,
        animated,
    }))
}

async fn emoji_guild(db: &PgPool, emoji_id: i64) -> AppResult<(i64, String)> {
    sqlx::query_as::<_, (i64, String)>("SELECT guild_id, storage_key FROM emojis WHERE id = $1")
        .bind(emoji_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub struct RenameEmoji {
    name: String,
}

/// `PATCH /api/emojis/{id}` `{"name": "..."}`. Old messages keep working.
pub async fn rename(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(emoji_id): Path<i64>,
    Json(req): Json<RenameEmoji>,
) -> AppResult<Json<CustomEmoji>> {
    let (guild_id, _) = emoji_guild(&st.db, emoji_id).await?;
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let name = clean_name(&req.name)?;
    check_name_free(&st.db, guild_id, &name).await?;
    let row = sqlx::query_as::<_, EmojiRow>(
        "UPDATE emojis SET name = $2 WHERE id = $1 RETURNING id, guild_id, name, animated",
    )
    .bind(emoji_id)
    .bind(&name)
    .fetch_one(&st.db)
    .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::EmojisChanged { guild_id },
    );
    Ok(Json(row.into()))
}

/// `DELETE /api/emojis/{id}`
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(emoji_id): Path<i64>,
) -> AppResult<StatusCode> {
    let (guild_id, key) = emoji_guild(&st.db, emoji_id).await?;
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    sqlx::query("DELETE FROM emojis WHERE id = $1")
        .bind(emoji_id)
        .execute(&st.db)
        .await?;
    let _ = tokio::fs::remove_file(st.config.upload_dir.join(key)).await;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::EmojisChanged { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("raid_boss"));
        assert!(valid_name("pog2"));
        assert!(!valid_name("a"));
        assert!(!valid_name("12"));
        assert!(!valid_name("Caps"));
        assert!(!valid_name("has-dash"));
    }

    #[test]
    fn tokens() {
        assert_eq!(parse_token("<:pog:123>"), Some(("pog", 123)));
        assert_eq!(parse_token("<:pog:12a>"), None);
        assert_eq!(parse_token("<:pog:123> "), None);
        assert_eq!(parse_token("<@123>"), None);
    }

    #[test]
    fn shortcodes_are_replaced() {
        let lookup = |n: &str| match n {
            "pog" => Some(1),
            "kek" => Some(2),
            _ => None,
        };
        assert_eq!(
            replace_shortcodes("gg :pog: :nope:", lookup),
            "gg <:pog:1> :nope:"
        );
        assert_eq!(replace_shortcodes(":pog::kek:", lookup), "<:pog:1><:kek:2>");
        assert_eq!(
            replace_shortcodes("<:pog:9> stays", lookup),
            "<:pog:9> stays"
        );
        assert_eq!(
            replace_shortcodes("`:pog:` :pog:", lookup),
            "`:pog:` <:pog:1>"
        );
        assert_eq!(
            replace_shortcodes("at 12:30:45 ok", lookup),
            "at 12:30:45 ok"
        );
        assert_eq!(
            replace_shortcodes("héllo :kek: 👍", lookup),
            "héllo <:kek:2> 👍"
        );
    }
}
