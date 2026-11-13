//! Servers ("outhouses"): create, list, leave, delete. Invites are in `invites.rs`.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, CreateGuildRequest, Guild, MyPermissions};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{self, member_ctx};
use crate::state::{AppState, Audience};

/// The columns of a `Guild` (from `guilds g`).
pub const GUILD_COLUMNS: &str = "g.id, g.name, g.owner_id, g.created_at,
    CASE WHEN g.icon_key IS NULL THEN NULL ELSE '/files/' || g.icon_key || '/icon' END AS icon_url";

pub async fn guilds_for_user(db: &PgPool, user_id: i64) -> AppResult<Vec<Guild>> {
    let guilds = sqlx::query_as::<_, Guild>(&format!(
        "SELECT {GUILD_COLUMNS}
         FROM guilds g JOIN guild_members m ON m.guild_id = g.id
         WHERE m.user_id = $1
         ORDER BY m.joined_at"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(guilds)
}

pub async fn fetch_guild(db: &PgPool, guild_id: i64) -> AppResult<Guild> {
    sqlx::query_as::<_, Guild>(&format!(
        "SELECT {GUILD_COLUMNS} FROM guilds g WHERE g.id = $1"
    ))
    .bind(guild_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Guild>>> {
    Ok(Json(guilds_for_user(&st.db, user.id).await?))
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CreateGuildRequest>,
) -> AppResult<Json<Guild>> {
    let name = req.name.trim();
    if !(2..=64).contains(&name.chars().count()) {
        return Err(AppError::BadRequest(
            "Server names are 2–64 characters.".into(),
        ));
    }

    let mut tx = st.db.begin().await?;
    let guild = sqlx::query_as::<_, Guild>(
        "INSERT INTO guilds (id, name, owner_id) VALUES ($1, $2, $3)
         RETURNING id, name, owner_id, created_at",
    )
    .bind(st.ids.next_id())
    .bind(name)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO roles (id, guild_id, name, permissions, position)
         VALUES ($1, $1, '@everyone', $2, 0)",
    )
    .bind(guild.id)
    .bind(p::DEFAULT_EVERYONE as i64)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)")
        .bind(guild.id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    // Every new outhouse comes with a front porch and somewhere to talk.
    for (position, (channel_name, kind, topic)) in [
        (
            "front-porch",
            channel_kind::TEXT,
            "Leave it cleaner than you found it.",
        ),
        ("The Long Drop", channel_kind::VOICE, ""),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO channels (id, guild_id, name, kind, position, topic)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(st.ids.next_id())
        .bind(guild.id)
        .bind(channel_name)
        .bind(kind)
        .bind(position as i32)
        .bind(topic)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    st.dispatch(
        Audience::User(user.id),
        ServerEvent::GuildCreate(guild.clone()),
    );
    Ok(Json(guild))
}

/// Deletes the whole outhouse. Owner only.
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<StatusCode> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    if !ctx.is_owner() {
        return Err(AppError::Forbidden);
    }
    let member_ids: Vec<i64> =
        sqlx::query_scalar("SELECT user_id FROM guild_members WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_all(&st.db)
            .await?;
    sqlx::query("DELETE FROM guilds WHERE id = $1")
        .bind(guild_id)
        .execute(&st.db)
        .await?;
    for id in member_ids {
        st.dispatch(Audience::User(id), ServerEvent::GuildDelete { guild_id });
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Leave an outhouse (the owner has to delete it instead).
pub async fn leave(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<StatusCode> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    if ctx.is_owner() {
        return Err(AppError::BadRequest(
            "You own this outhouse. Delete it instead, or hand it over first.".into(),
        ));
    }
    remove_member(&st, guild_id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes someone from a server and tells everyone.
pub async fn remove_member(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<()> {
    super::voice::clear_voice(st, user_id, Some(guild_id));
    sqlx::query("DELETE FROM guild_members WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id)
        .bind(user_id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::MemberLeave { guild_id, user_id },
    );
    st.dispatch(
        Audience::User(user_id),
        ServerEvent::GuildDelete { guild_id },
    );
    Ok(())
}

/// Your permissions in the server and in each channel, for the client UI.
pub async fn my_permissions(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<MyPermissions>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    let channel_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    let mut channels = HashMap::new();
    for id in channel_ids {
        let bits = perms::channel_perms(&st.db, &ctx, id).await?;
        channels.insert(id.to_string(), bits.to_string());
    }
    Ok(Json(MyPermissions {
        guild: ctx.guild_perms() as i64,
        channels,
        top_position: ctx.top_position,
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct PreviewQuery {
    /// Role IDs, comma separated (empty: @everyone alone).
    #[serde(default)]
    pub roles: String,
}

/// `GET /api/guilds/{id}/preview-permissions?roles=a,b` ("View outhouse as
/// role", Manage Roles): what someone with only @everyone and these roles
/// could do, worked out by the same rules as for a real member. Only
/// channels you can see yourself are listed.
pub async fn preview_permissions(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    axum::extract::Query(q): axum::extract::Query<PreviewQuery>,
) -> AppResult<Json<MyPermissions>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    ctx.require(p::MANAGE_ROLES)?;
    let mut role_ids = Vec::new();
    for part in q.roles.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let id: i64 = part
            .parse()
            .map_err(|_| AppError::BadRequest("Bad role ID.".into()))?;
        if !role_ids.contains(&id) {
            role_ids.push(id);
        }
    }
    if role_ids.len() > 25 {
        return Err(AppError::BadRequest(
            "Preview up to 25 roles at once.".into(),
        ));
    }
    let preview = perms::role_set_ctx(&st.db, guild_id, &role_ids).await?;
    let channel_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    let mut channels = HashMap::new();
    for id in channel_ids {
        let overwrites = perms::overwrites(&st.db, id).await?;
        if perms::channel_perms_with(&ctx, &overwrites) & p::VIEW_CHANNEL == 0 {
            continue;
        }
        let bits = perms::channel_perms_with(&preview, &overwrites);
        channels.insert(id.to_string(), bits.to_string());
    }
    Ok(Json(MyPermissions {
        guild: preview.guild_perms() as i64,
        channels,
        top_position: preview.top_position,
    }))
}
