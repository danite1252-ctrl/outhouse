//! Webhooks: let other services post into a channel.
//!
//! `POST /api/webhooks/{id}/{token}` accepts Discord's webhook JSON
//! (content, username, avatar_url, embeds), so anything that can "post to a
//! Discord webhook" works with an Outhouse URL too. Add `?wait=true` to get
//! the created message back.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use outhouse_common::models::{CreateWebhookRequest, Embed, EmbedField, Webhook};
use outhouse_common::permissions as p;
use serde::Deserialize;

use super::messages::post_bot_message;
use crate::auth::{self, AuthUser};
use crate::embeds::clip;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access_meta as channel_access, member_ctx};
use crate::state::AppState;

const TOKEN_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn http_url(s: Option<String>) -> Option<String> {
    s.map(|u| u.trim().to_string())
        .filter(|u| (u.starts_with("https://") || u.starts_with("http://")) && u.len() <= 2048)
}

// ---- Managing webhooks ---------------------------------------------------

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Webhook>>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_WEBHOOKS)?;
    let hooks = sqlx::query_as::<_, Webhook>(
        "SELECT id, guild_id, channel_id, name, token, created_at
         FROM webhooks WHERE guild_id = $1 ORDER BY created_at",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(hooks))
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateWebhookRequest>,
) -> AppResult<Json<Webhook>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_WEBHOOKS)?;
    let name: String = req.name.trim().chars().take(32).collect();
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the webhook a name.".into()));
    }
    let id = st.ids.next_id();
    let bot_id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    auth::create_bot_user(&mut *tx, bot_id, &name).await?;
    let hook = sqlx::query_as::<_, Webhook>(
        "INSERT INTO webhooks (id, guild_id, channel_id, user_id, name, token, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, guild_id, channel_id, name, token, created_at",
    )
    .bind(id)
    .bind(access.channel.guild_id)
    .bind(channel_id)
    .bind(bot_id)
    .bind(&name)
    .bind(auth::random_code(48, TOKEN_ALPHABET))
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    crate::audit::Entry::new(access.channel.guild_id, user.id, "webhook_create")
        .target("webhook", hook.id, name.clone())
        .details(format!("Posts in #{}", access.channel.name))
        .save(&st)
        .await;
    Ok(Json(hook))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(webhook_id): Path<i64>,
) -> AppResult<StatusCode> {
    let guild_id: i64 = sqlx::query_scalar("SELECT guild_id FROM webhooks WHERE id = $1")
        .bind(webhook_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_WEBHOOKS)?;
    let name: Option<String> =
        sqlx::query_scalar("DELETE FROM webhooks WHERE id = $1 RETURNING name")
            .bind(webhook_id)
            .fetch_optional(&st.db)
            .await?;
    crate::audit::Entry::new(guild_id, user.id, "webhook_delete")
        .target("webhook", webhook_id, name.unwrap_or_default())
        .save(&st)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Executing (Discord-compatible) ---------------------------------------

#[derive(Deserialize, Default)]
pub struct DiscordPayload {
    content: Option<String>,
    username: Option<String>,
    avatar_url: Option<String>,
    #[serde(default)]
    embeds: Vec<DiscordEmbed>,
}

#[derive(Deserialize, Default)]
struct DiscordEmbed {
    title: Option<String>,
    description: Option<String>,
    url: Option<String>,
    color: Option<i64>,
    timestamp: Option<String>,
    author: Option<NameUrlIcon>,
    footer: Option<FooterPart>,
    thumbnail: Option<UrlPart>,
    image: Option<UrlPart>,
    #[serde(default)]
    fields: Vec<FieldPart>,
}

#[derive(Deserialize, Default)]
struct NameUrlIcon {
    name: Option<String>,
    url: Option<String>,
    icon_url: Option<String>,
}

#[derive(Deserialize, Default)]
struct FooterPart {
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct UrlPart {
    url: Option<String>,
}

#[derive(Deserialize, Default)]
struct FieldPart {
    name: Option<String>,
    value: Option<String>,
    #[serde(default)]
    inline: bool,
}

fn convert_embed(e: DiscordEmbed) -> Embed {
    let text = |s: Option<String>, max: usize| s.map(|s| clip(s, max)).filter(|s| !s.is_empty());
    let author = e.author.unwrap_or_default();
    Embed {
        url: http_url(e.url).unwrap_or_default(),
        kind: "rich".into(),
        title: text(e.title, 256),
        description: text(e.description, 4096),
        site_name: None,
        image_url: http_url(e.image.and_then(|i| i.url)),
        color: e.color.map(|c| (c & 0xFF_FF_FF) as i32),
        author_name: text(author.name, 256),
        author_url: http_url(author.url),
        author_icon_url: http_url(author.icon_url),
        footer: text(e.footer.and_then(|f| f.text), 2048),
        thumbnail_url: http_url(e.thumbnail.and_then(|t| t.url)),
        timestamp: text(e.timestamp, 40),
        fields: e
            .fields
            .into_iter()
            .take(25)
            .filter_map(|f| {
                Some(EmbedField {
                    name: clip(f.name?, 256),
                    value: clip(f.value?, 1024),
                    inline: f.inline,
                })
            })
            .collect(),
    }
}

#[derive(Deserialize)]
pub struct ExecuteQuery {
    wait: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct HookRow {
    guild_id: i64,
    channel_id: i64,
    user_id: i64,
    token: String,
}

pub async fn execute(
    State(st): State<AppState>,
    Path((webhook_id, token)): Path<(i64, String)>,
    Query(q): Query<ExecuteQuery>,
    Json(payload): Json<DiscordPayload>,
) -> AppResult<Response> {
    let hook = sqlx::query_as::<_, HookRow>(
        "SELECT guild_id, channel_id, user_id, token FROM webhooks WHERE id = $1",
    )
    .bind(webhook_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // Compare without bailing early, so timing doesn't leak the token.
    let matches = hook.token.len() == token.len()
        && hook
            .token
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !matches {
        return Err(AppError::NotFound);
    }

    let content = payload.content.unwrap_or_default().trim().to_string();
    if content.chars().count() > 4000 {
        return Err(AppError::BadRequest(
            "content is over 4000 characters.".into(),
        ));
    }
    if payload.embeds.len() > 10 {
        return Err(AppError::BadRequest("Up to 10 embeds per message.".into()));
    }
    let embeds: Vec<Embed> = payload.embeds.into_iter().map(convert_embed).collect();
    if content.is_empty() && embeds.is_empty() {
        return Err(AppError::BadRequest("Send content or embeds.".into()));
    }
    let username = payload
        .username
        .map(|u| u.trim().chars().take(80).collect::<String>())
        .filter(|u| !u.is_empty());

    let message = post_bot_message(
        &st,
        hook.guild_id,
        hook.channel_id,
        hook.user_id,
        username,
        http_url(payload.avatar_url),
        content,
        embeds,
    )
    .await?;

    if q.wait.unwrap_or(false) {
        Ok(Json(message).into_response())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_discord_embeds() {
        let payload: DiscordPayload = serde_json::from_str(
            r#"{
                "content": "Raid tonight",
                "embeds": [{
                    "title": "Nerub-ar Palace",
                    "url": "https://raider.io",
                    "color": 16737077,
                    "fields": [{"name": "Time", "value": "8pm", "inline": true}, {"name": "No value"}],
                    "thumbnail": {"url": "javascript:alert(1)"},
                    "unknown_field": 1
                }]
            }"#,
        )
        .unwrap();
        let e = convert_embed(payload.embeds.into_iter().next().unwrap());
        assert_eq!(e.kind, "rich");
        assert_eq!(e.color, Some(0xFF6335));
        assert_eq!(e.fields.len(), 1);
        assert!(e.fields[0].inline);
        assert_eq!(e.thumbnail_url, None, "non-http URLs are dropped");
    }
}
