//! Roles: list, create, edit, delete.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{CreateRoleRequest, Role, UpdateRoleRequest};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::{AppState, Audience};

async fn fetch_role(db: &PgPool, guild_id: i64, role_id: i64) -> AppResult<Role> {
    sqlx::query_as::<_, Role>(
        "SELECT id, guild_id, name, color, permissions, deny_permissions, position, hoist
         FROM roles WHERE id = $1 AND guild_id = $2",
    )
    .bind(role_id)
    .bind(guild_id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

fn clean_name(raw: &str) -> AppResult<String> {
    let name: String = raw.trim().chars().take(32).collect();
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the role a name.".into()));
    }
    Ok(name)
}

fn clean_color(c: i32) -> i32 {
    c.clamp(0, 0xFF_FF_FF)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Role>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let roles = sqlx::query_as::<_, Role>(
        "SELECT id, guild_id, name, color, permissions, deny_permissions, position, hoist
         FROM roles WHERE guild_id = $1 ORDER BY position DESC, id",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(roles))
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<CreateRoleRequest>,
) -> AppResult<Json<Role>> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    actor.require(p::MANAGE_ROLES)?;
    let perms = (req.permissions.unwrap_or(0) as u64) & p::ALL;
    let deny = (req.deny_permissions.unwrap_or(0) as u64) & p::ALL & !perms;
    actor.require_can_grant(perms)?;
    let name = clean_name(&req.name)?;

    // New roles go in at the bottom (just above @everyone), so anyone who
    // can manage roles can manage the one they just made.
    let mut tx = st.db.begin().await?;
    sqlx::query("UPDATE roles SET position = position + 1 WHERE guild_id = $1 AND position >= 1")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    let role = sqlx::query_as::<_, Role>(
        "INSERT INTO roles (id, guild_id, name, color, permissions, deny_permissions, position, hoist)
         VALUES ($1, $2, $3, $4, $5, $7, 1, $6)
         RETURNING id, guild_id, name, color, permissions, deny_permissions, position, hoist",
    )
    .bind(st.ids.next_id())
    .bind(guild_id)
    .bind(&name)
    .bind(clean_color(req.color.unwrap_or(0)))
    .bind(perms as i64)
    .bind(req.hoist.unwrap_or(false))
    .bind(deny as i64)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(Json(role))
}

pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, role_id)): Path<(i64, i64)>,
    Json(req): Json<UpdateRoleRequest>,
) -> AppResult<Json<Role>> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    actor.require(p::MANAGE_ROLES)?;
    let role = fetch_role(&st.db, guild_id, role_id).await?;
    let is_everyone = role.id == guild_id;
    if !is_everyone && !actor.outranks(role.position) {
        return Err(AppError::BadRequest(
            "That role is ranked the same as your top role or higher.".into(),
        ));
    }

    let name = match &req.name {
        Some(n) if !is_everyone => clean_name(n)?,
        _ => role.name.clone(),
    };
    let permissions = match req.permissions {
        Some(bits) => {
            let bits = (bits as u64) & p::ALL;
            // Only the newly added bits need checking.
            actor.require_can_grant(bits & !(role.permissions as u64))?;
            bits as i64
        }
        None => role.permissions,
    };
    // @everyone has nothing to inherit from, so it can't deny.
    let deny_permissions = if is_everyone {
        0
    } else {
        let bits = req
            .deny_permissions
            .map(|d| d as u64)
            .unwrap_or(role.deny_permissions as u64);
        (bits & p::ALL & !(permissions as u64)) as i64
    };
    let position = match req.position {
        Some(pos) if !is_everyone => {
            let pos = pos.max(1);
            if !actor.outranks(pos) {
                return Err(AppError::BadRequest(
                    "You can't move a role above your own top role.".into(),
                ));
            }
            pos
        }
        _ => role.position,
    };

    let updated = sqlx::query_as::<_, Role>(
        "UPDATE roles SET name = $3, color = $4, permissions = $5, hoist = $6, position = $7,
                          deny_permissions = $8
         WHERE id = $1 AND guild_id = $2
         RETURNING id, guild_id, name, color, permissions, deny_permissions, position, hoist",
    )
    .bind(role_id)
    .bind(guild_id)
    .bind(&name)
    .bind(clean_color(req.color.unwrap_or(role.color)))
    .bind(permissions)
    .bind(req.hoist.unwrap_or(role.hoist))
    .bind(position)
    .bind(deny_permissions)
    .fetch_one(&st.db)
    .await?;

    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(Json(updated))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, role_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    actor.require(p::MANAGE_ROLES)?;
    if role_id == guild_id {
        return Err(AppError::BadRequest("@everyone can't be deleted.".into()));
    }
    let role = fetch_role(&st.db, guild_id, role_id).await?;
    if !actor.outranks(role.position) {
        return Err(AppError::BadRequest(
            "That role is ranked the same as your top role or higher.".into(),
        ));
    }
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM channel_overwrites WHERE target_id = $1 AND target_kind = 0")
        .bind(role_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM roles WHERE id = $1")
        .bind(role_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    Ok(StatusCode::NO_CONTENT)
}
