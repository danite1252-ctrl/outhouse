//! Channels: list, create, delete, permission overwrites, read state.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, AckRequest, Channel, CreateChannelRequest, PermissionOverwrite,
    SetOverwriteRequest,
};
use outhouse_common::permissions as p;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{self, channel_access, member_ctx};
use crate::state::{AppState, Audience};

/// Channels in a server that you can see, with your unread state.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Channel>>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let channels = sqlx::query_as::<_, Channel>(
        "SELECT c.id, c.guild_id, c.name, c.kind, c.position, c.last_message_id,
                rs.last_read_id
         FROM channels c
         LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $2
         WHERE c.guild_id = $1
         ORDER BY c.position, c.id",
    )
    .bind(guild_id)
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;

    let mut visible = Vec::with_capacity(channels.len());
    for channel in channels {
        let bits = perms::channel_perms(&st.db, &ctx, channel.id).await?;
        if bits & p::VIEW_CHANNEL != 0 {
            visible.push(channel);
        }
    }
    Ok(Json(visible))
}

/// Text channel names look like `rust-help`: lower case, dashes for spaces.
fn normalise_text_channel_name(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(50)
        .collect()
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<CreateChannelRequest>,
) -> AppResult<Json<Channel>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_CHANNELS)?;
    let kind = req.kind.unwrap_or(channel_kind::TEXT);
    let name = match kind {
        channel_kind::TEXT => normalise_text_channel_name(&req.name),
        channel_kind::VOICE => req.name.trim().chars().take(50).collect(),
        _ => return Err(AppError::BadRequest("Unknown channel type.".into())),
    };
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the channel a name.".into()));
    }

    let channel = sqlx::query_as::<_, Channel>(
        "INSERT INTO channels (id, guild_id, name, kind, position)
         VALUES ($1, $2, $3, $4,
                 (SELECT COALESCE(MAX(position) + 1, 0) FROM channels WHERE guild_id = $2))
         RETURNING id, guild_id, name, kind, position, last_message_id, NULL::BIGINT AS last_read_id",
    )
    .bind(st.ids.next_id())
    .bind(guild_id)
    .bind(&name)
    .bind(kind)
    .fetch_one(&st.db)
    .await?;

    st.dispatch(
        Audience::Channel {
            guild_id,
            channel_id: channel.id,
        },
        ServerEvent::ChannelCreate(channel.clone()),
    );
    Ok(Json(channel))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::MANAGE_CHANNELS)?;
    let guild_id = access.channel.guild_id;
    sqlx::query("DELETE FROM channels WHERE id = $1")
        .bind(channel_id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::ChannelDelete {
            id: channel_id,
            guild_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Marks a channel as read up to a message.
pub async fn ack(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<AckRequest>,
) -> AppResult<StatusCode> {
    channel_access(&st.db, channel_id, user.id).await?;
    sqlx::query(
        "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id)
         DO UPDATE SET last_read_id = GREATEST(read_states.last_read_id, EXCLUDED.last_read_id)",
    )
    .bind(user.id)
    .bind(channel_id)
    .bind(req.message_id)
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Permission overwrites ---------------------------------------------

#[derive(sqlx::FromRow)]
struct OverwriteRow {
    target_id: i64,
    target_kind: i16,
    allow_bits: i64,
    deny_bits: i64,
}

pub async fn list_overwrites(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<Json<Vec<PermissionOverwrite>>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let rows = sqlx::query_as::<_, OverwriteRow>(
        "SELECT target_id, target_kind, allow_bits, deny_bits
         FROM channel_overwrites WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PermissionOverwrite {
                target_id: r.target_id,
                kind: if r.target_kind == 1 { "member" } else { "role" }.into(),
                allow: r.allow_bits,
                deny: r.deny_bits,
            })
            .collect(),
    ))
}

pub async fn set_overwrite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, target_id)): Path<(i64, i64)>,
    Json(req): Json<SetOverwriteRequest>,
) -> AppResult<Json<PermissionOverwrite>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let guild_id = access.channel.guild_id;
    let target_kind: i16 = match req.kind.as_str() {
        "role" => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM roles WHERE id = $1 AND guild_id = $2)",
            )
            .bind(target_id)
            .bind(guild_id)
            .fetch_one(&st.db)
            .await?;
            if !exists {
                return Err(AppError::NotFound);
            }
            0
        }
        "member" => {
            member_ctx(&st.db, guild_id, target_id).await?;
            1
        }
        _ => return Err(AppError::BadRequest("kind must be role or member.".into())),
    };
    let allow = (req.allow as u64) & p::ALL;
    let deny = (req.deny as u64) & p::ALL & !allow;
    access.ctx.require_can_grant(allow | deny)?;

    sqlx::query(
        "INSERT INTO channel_overwrites (channel_id, target_id, target_kind, allow_bits, deny_bits)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (channel_id, target_id)
         DO UPDATE SET target_kind = EXCLUDED.target_kind,
                       allow_bits = EXCLUDED.allow_bits,
                       deny_bits = EXCLUDED.deny_bits",
    )
    .bind(channel_id)
    .bind(target_id)
    .bind(target_kind)
    .bind(allow as i64)
    .bind(deny as i64)
    .execute(&st.db)
    .await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(Json(PermissionOverwrite {
        target_id,
        kind: req.kind,
        allow: allow as i64,
        deny: deny as i64,
    }))
}

pub async fn delete_overwrite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, target_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.ctx.require(p::MANAGE_ROLES)?;
    let guild_id = access.channel.guild_id;
    sqlx::query("DELETE FROM channel_overwrites WHERE channel_id = $1 AND target_id = $2")
        .bind(channel_id)
        .bind(target_id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}
