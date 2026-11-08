//! Outhouse settings → Raids: which roles are raid managers, and the channel
//! "Plan a raid" starts on.
//!
//! A raid manager runs every raid in the outhouse (moves people between
//! roster and bench, removes sign-ups, takes attendance, edits and cancels
//! raids, exports rosters) without needing Manage Events.

use axum::extract::{Path, State};
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, RaidSettings};
use outhouse_common::permissions as p;
use sqlx::PgPool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, MemberCtx};
use crate::state::{AppState, Audience};

/// Most roles that can be raid managers at once.
const MAX_MANAGER_ROLES: usize = 25;

pub async fn load(db: &PgPool, guild_id: i64) -> AppResult<RaidSettings> {
    let manager_role_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT m.role_id FROM raid_manager_roles m JOIN roles r ON r.id = m.role_id
         WHERE m.guild_id = $1 ORDER BY r.position DESC, r.id",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    let default_channel_id: Option<i64> =
        sqlx::query_scalar("SELECT default_channel_id FROM raid_settings WHERE guild_id = $1")
            .bind(guild_id)
            .fetch_optional(db)
            .await?
            .flatten();
    Ok(RaidSettings {
        manager_role_ids,
        default_channel_id,
    })
}

/// Has one of the outhouse's raid manager roles (timed-out members don't
/// count).
pub async fn has_manager_role(db: &PgPool, ctx: &MemberCtx) -> AppResult<bool> {
    if ctx.timed_out || ctx.role_ids.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM raid_manager_roles WHERE guild_id = $1 AND role_id = ANY($2))",
    )
    .bind(ctx.guild_id)
    .bind(&ctx.role_ids)
    .fetch_one(db)
    .await?)
}

/// `GET /api/guilds/{id}/raid-settings` (any member: the client works out
/// who manages raids from it).
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<RaidSettings>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    Ok(Json(load(&st.db, guild_id).await?))
}

async fn role_names(db: &PgPool, ids: &[i64]) -> AppResult<String> {
    if ids.is_empty() {
        return Ok("nobody".into());
    }
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM roles WHERE id = ANY($1) ORDER BY position DESC, id")
            .bind(ids)
            .fetch_all(db)
            .await?;
    Ok(names.join(", "))
}

async fn channel_label(db: &PgPool, id: Option<i64>) -> AppResult<String> {
    let Some(id) = id else {
        return Ok("the channel you're in".into());
    };
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM channels WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(format!(
        "#{}",
        name.unwrap_or_else(|| "deleted-channel".into())
    ))
}

/// `PUT /api/guilds/{id}/raid-settings` (needs Manage Outhouse).
pub async fn set(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<RaidSettings>,
) -> AppResult<Json<RaidSettings>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let mut role_ids = req.manager_role_ids.clone();
    role_ids.sort_unstable();
    role_ids.dedup();
    if role_ids.len() > MAX_MANAGER_ROLES {
        return Err(AppError::BadRequest(format!(
            "Up to {MAX_MANAGER_ROLES} raid manager roles."
        )));
    }
    if role_ids.contains(&guild_id) {
        return Err(AppError::BadRequest(
            "@everyone can't be a raid manager role. Make a role for your raid leaders instead."
                .into(),
        ));
    }
    let found: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roles WHERE guild_id = $1 AND id = ANY($2)")
            .bind(guild_id)
            .bind(&role_ids)
            .fetch_one(&st.db)
            .await?;
    if found as usize != role_ids.len() {
        return Err(AppError::BadRequest(
            "One of those roles isn't in this outhouse.".into(),
        ));
    }
    if let Some(channel_id) = req.default_channel_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id = $1 AND guild_id = $2 AND kind = $3)",
        )
        .bind(channel_id)
        .bind(guild_id)
        .bind(channel_kind::TEXT)
        .fetch_one(&st.db)
        .await?;
        if !ok {
            return Err(AppError::BadRequest(
                "Raid sign-ups go in one of this outhouse's text channels.".into(),
            ));
        }
    }

    let before = load(&st.db, guild_id).await?;
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM raid_manager_roles WHERE guild_id = $1")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO raid_manager_roles (guild_id, role_id) SELECT $1, unnest($2::BIGINT[])",
    )
    .bind(guild_id)
    .bind(&role_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO raid_settings (guild_id, default_channel_id) VALUES ($1, $2)
         ON CONFLICT (guild_id) DO UPDATE SET default_channel_id = EXCLUDED.default_channel_id",
    )
    .bind(guild_id)
    .bind(req.default_channel_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let after = load(&st.db, guild_id).await?;

    let mut details = Vec::new();
    let (mut old_roles, mut new_roles) = (
        before.manager_role_ids.clone(),
        after.manager_role_ids.clone(),
    );
    old_roles.sort_unstable();
    new_roles.sort_unstable();
    if old_roles != new_roles {
        details.push(format!(
            "Raid managers: {} → {}",
            role_names(&st.db, &before.manager_role_ids).await?,
            role_names(&st.db, &after.manager_role_ids).await?
        ));
    }
    if before.default_channel_id != after.default_channel_id {
        details.push(format!(
            "Sign-up channel: {} → {}",
            channel_label(&st.db, before.default_channel_id).await?,
            channel_label(&st.db, after.default_channel_id).await?
        ));
    }
    if !details.is_empty() {
        crate::audit::Entry::new(guild_id, user.id, "raid_settings_update")
            .details(details.join(" · "))
            .save(&st)
            .await;
    }
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::RaidSettingsUpdate {
            guild_id,
            settings: after.clone(),
        },
    );
    Ok(Json(after))
}
