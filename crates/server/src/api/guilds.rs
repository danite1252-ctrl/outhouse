//! Servers ("outhouses"): create, list, join, leave, delete, invites.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, CreateGuildRequest, Guild, InviteResponse, MyPermissions,
};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use super::members::fetch_member;
use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::perms::{self, member_ctx};
use crate::state::{AppState, Audience};

const INVITE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub async fn guilds_for_user(db: &PgPool, user_id: i64) -> AppResult<Vec<Guild>> {
    let guilds = sqlx::query_as::<_, Guild>(
        "SELECT g.id, g.name, g.owner_id, g.created_at
         FROM guilds g JOIN guild_members m ON m.guild_id = g.id
         WHERE m.user_id = $1
         ORDER BY m.joined_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(guilds)
}

pub async fn fetch_guild(db: &PgPool, guild_id: i64) -> AppResult<Guild> {
    sqlx::query_as::<_, Guild>("SELECT id, name, owner_id, created_at FROM guilds WHERE id = $1")
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

pub async fn create_invite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<InviteResponse>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::CREATE_INVITE)?;
    let code = auth::random_code(8, INVITE_ALPHABET);
    sqlx::query(
        "INSERT INTO invites (code, guild_id, creator_id, expires_at)
         VALUES ($1, $2, $3, now() + interval '7 days')",
    )
    .bind(&code)
    .bind(guild_id)
    .bind(user.id)
    .execute(&st.db)
    .await?;
    Ok(Json(InviteResponse { code }))
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    guild_id: i64,
    usable: bool,
}

pub async fn join_invite(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(code): Path<String>,
) -> AppResult<Json<Guild>> {
    let code = code.trim();
    let invite = sqlx::query_as::<_, InviteRow>(
        "SELECT guild_id,
                (expires_at IS NULL OR expires_at > now())
                AND (max_uses IS NULL OR uses < max_uses) AS usable
         FROM invites WHERE code = $1",
    )
    .bind(code)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if !invite.usable {
        return Err(AppError::BadRequest(
            "That invite has expired or been used up.".into(),
        ));
    }
    let banned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bans WHERE guild_id = $1 AND user_id = $2)",
    )
    .bind(invite.guild_id)
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if banned {
        return Err(AppError::BadRequest(
            "You've been banned from that outhouse.".into(),
        ));
    }

    let joined = sqlx::query(
        "INSERT INTO guild_members (guild_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(invite.guild_id)
    .bind(user.id)
    .execute(&st.db)
    .await?
    .rows_affected()
        == 1;

    let guild = fetch_guild(&st.db, invite.guild_id).await?;
    if joined {
        sqlx::query("UPDATE invites SET uses = uses + 1 WHERE code = $1")
            .bind(code)
            .execute(&st.db)
            .await?;
        let mut member = fetch_member(&st.db, guild.id, user.id).await?;
        member.online = st.is_online(user.id);
        st.dispatch(
            Audience::Guild(guild.id),
            ServerEvent::MemberJoin {
                guild_id: guild.id,
                member,
            },
        );
        st.dispatch(
            Audience::User(user.id),
            ServerEvent::GuildCreate(guild.clone()),
        );
    }
    Ok(Json(guild))
}
