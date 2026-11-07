//! Members: list, roles, kick, ban, timeout.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{BanRequest, Member, NicknameRequest, TimeoutRequest, User};
use outhouse_common::permissions as p;
use serde::Serialize;
use sqlx::PgPool;

use super::account::clean_name;
use super::guilds::remove_member;
use crate::audit;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{member_ctx, MemberCtx};
use crate::state::{AppState, Audience};

const MEMBER_SELECT: &str = "
    SELECT u.id, u.username,
           COALESCE(m.nickname, u.display_name) AS display_name,
           COALESCE(array_agg(mr.role_id) FILTER (WHERE mr.role_id IS NOT NULL), '{}') AS role_ids,
           m.joined_at, m.timeout_until, false AS online,
           CASE WHEN u.avatar_key IS NULL THEN NULL ELSE '/files/' || u.avatar_key || '/avatar' END AS avatar_url,
           u.is_bot AS bot
    FROM guild_members m
    JOIN users u ON u.id = m.user_id
    LEFT JOIN member_roles mr ON mr.guild_id = m.guild_id AND mr.user_id = m.user_id
    WHERE m.guild_id = $1";

/// Fills in who's online (presence lives in memory, not the database).
fn with_presence(st: &AppState, mut members: Vec<Member>) -> Vec<Member> {
    for m in &mut members {
        m.online = st.is_online(m.id);
    }
    members
}

pub async fn fetch_member(db: &PgPool, guild_id: i64, user_id: i64) -> AppResult<Member> {
    let sql = format!(
        "{MEMBER_SELECT} AND m.user_id = $2
         GROUP BY u.id, m.nickname, m.joined_at, m.timeout_until"
    );
    sqlx::query_as::<_, Member>(&sql)
        .bind(guild_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<Member>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let sql = format!(
        "{MEMBER_SELECT}
         GROUP BY u.id, m.nickname, m.joined_at, m.timeout_until
         ORDER BY lower(COALESCE(m.nickname, u.display_name))"
    );
    let members = sqlx::query_as::<_, Member>(&sql)
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(Json(with_presence(&st, members)))
}

/// Checks `actor` may act on `target` with `perm`: has the permission,
/// isn't targeting themselves or the owner, and outranks them.
async fn moderate(
    db: &PgPool,
    actor: &MemberCtx,
    target_id: i64,
    perm: u64,
) -> AppResult<MemberCtx> {
    actor.require(perm)?;
    if target_id == actor.user_id {
        return Err(AppError::BadRequest(
            "You can't do that to yourself.".into(),
        ));
    }
    let target = member_ctx(db, actor.guild_id, target_id).await?;
    if target.is_owner() || !actor.outranks(target.top_position) {
        return Err(AppError::BadRequest(
            "They're ranked the same as you or higher.".into(),
        ));
    }
    Ok(target)
}

pub async fn kick(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    moderate(&st.db, &actor, target_id, p::KICK_MEMBERS).await?;
    let name = audit::user_label(&st, guild_id, target_id).await;
    remove_member(&st, guild_id, target_id).await?;
    audit::Entry::new(guild_id, user.id, "member_kick")
        .target("member", target_id, name)
        .save(&st)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn ban(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
    Json(req): Json<BanRequest>,
) -> AppResult<StatusCode> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    moderate(&st.db, &actor, target_id, p::BAN_MEMBERS).await?;
    let reason = req
        .reason
        .map(|r| r.trim().chars().take(500).collect::<String>())
        .filter(|r| !r.is_empty());
    sqlx::query(
        "INSERT INTO bans (guild_id, user_id, reason, banned_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET reason = EXCLUDED.reason",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(&reason)
    .bind(user.id)
    .execute(&st.db)
    .await?;
    let name = audit::user_label(&st, guild_id, target_id).await;
    remove_member(&st, guild_id, target_id).await?;
    audit::Entry::new(guild_id, user.id, "member_ban")
        .target("member", target_id, name)
        .details(reason.map(|r| format!("Reason: {r}")).unwrap_or_default())
        .save(&st)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct BanEntry {
    #[serde(flatten)]
    #[sqlx(flatten)]
    user: User,
    reason: Option<String>,
}

pub async fn list_bans(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<BanEntry>>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::BAN_MEMBERS)?;
    let bans = sqlx::query_as::<_, BanEntry>(
        "SELECT u.id, u.username, u.display_name, b.reason
         FROM bans b JOIN users u ON u.id = b.user_id
         WHERE b.guild_id = $1 ORDER BY b.created_at DESC",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(bans))
}

pub async fn unban(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::BAN_MEMBERS)?;
    let removed = sqlx::query("DELETE FROM bans WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id)
        .bind(target_id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if removed > 0 {
        let name = audit::user_label(&st, guild_id, target_id).await;
        audit::Entry::new(guild_id, user.id, "member_unban")
            .target("member", target_id, name)
            .save(&st)
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn timeout(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id)): Path<(i64, i64)>,
    Json(req): Json<TimeoutRequest>,
) -> AppResult<Json<Member>> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    moderate(&st.db, &actor, target_id, p::TIMEOUT_MEMBERS).await?;
    let minutes = req.minutes.unwrap_or(0).clamp(0, 28 * 24 * 60);
    sqlx::query(
        "UPDATE guild_members
         SET timeout_until = CASE WHEN $3 > 0 THEN now() + make_interval(mins => $3::INT) END
         WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(minutes)
    .execute(&st.db)
    .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    let mut member = fetch_member(&st.db, guild_id, target_id).await?;
    member.online = st.is_online(member.id);
    let entry = if minutes > 0 {
        audit::Entry::new(guild_id, user.id, "member_timeout")
            .details(format!("For {}", duration_label(minutes)))
    } else {
        audit::Entry::new(guild_id, user.id, "member_timeout_remove")
    };
    entry
        .target("member", target_id, member.display_name.clone())
        .save(&st)
        .await;
    Ok(Json(member))
}

/// "10 minutes", "1 hour", "7 days".
fn duration_label(minutes: i64) -> String {
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if minutes % (24 * 60) == 0 {
        plural(minutes / (24 * 60), "day")
    } else if minutes % 60 == 0 {
        plural(minutes / 60, "hour")
    } else {
        plural(minutes, "minute")
    }
}

/// Shared checks for giving or taking a role.
async fn check_role_change(
    st: &AppState,
    user_id: i64,
    guild_id: i64,
    target_id: i64,
    role_id: i64,
) -> AppResult<()> {
    let actor = member_ctx(&st.db, guild_id, user_id).await?;
    actor.require(p::MANAGE_ROLES)?;
    if role_id == guild_id {
        return Err(AppError::BadRequest(
            "Everyone has @everyone; it can't be given or taken.".into(),
        ));
    }
    let position: i32 =
        sqlx::query_scalar("SELECT position FROM roles WHERE id = $1 AND guild_id = $2")
            .bind(role_id)
            .bind(guild_id)
            .fetch_optional(&st.db)
            .await?
            .ok_or(AppError::NotFound)?;
    if !actor.outranks(position) {
        return Err(AppError::BadRequest(
            "That role is ranked the same as your top role or higher.".into(),
        ));
    }
    member_ctx(&st.db, guild_id, target_id).await?; // target must be a member
    Ok(())
}

pub async fn add_role(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id, role_id)): Path<(i64, i64, i64)>,
) -> AppResult<StatusCode> {
    check_role_change(&st, user.id, guild_id, target_id, role_id).await?;
    let added = sqlx::query(
        "INSERT INTO member_roles (guild_id, user_id, role_id) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(role_id)
    .execute(&st.db)
    .await?
    .rows_affected();
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    if added > 0 {
        log_role_change(
            &st,
            user.id,
            guild_id,
            target_id,
            role_id,
            "member_role_add",
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn log_role_change(
    st: &AppState,
    actor_id: i64,
    guild_id: i64,
    target_id: i64,
    role_id: i64,
    action: &'static str,
) {
    let role: Option<String> = sqlx::query_scalar("SELECT name FROM roles WHERE id = $1")
        .bind(role_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    let name = audit::user_label(st, guild_id, target_id).await;
    audit::Entry::new(guild_id, actor_id, action)
        .target("member", target_id, name)
        .details(format!("@{}", role.unwrap_or_else(|| "a role".into())))
        .save(st)
        .await;
}

pub async fn remove_role(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, target_id, role_id)): Path<(i64, i64, i64)>,
) -> AppResult<StatusCode> {
    check_role_change(&st, user.id, guild_id, target_id, role_id).await?;
    let removed = sqlx::query(
        "DELETE FROM member_roles WHERE guild_id = $1 AND user_id = $2 AND role_id = $3",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(role_id)
    .execute(&st.db)
    .await?
    .rows_affected();
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    if removed > 0 {
        log_role_change(
            &st,
            user.id,
            guild_id,
            target_id,
            role_id,
            "member_role_remove",
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Sets (or clears) your own nickname in an outhouse.
pub async fn set_nickname(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<NicknameRequest>,
) -> AppResult<Json<Member>> {
    let actor = member_ctx(&st.db, guild_id, user.id).await?;
    actor.require(p::CHANGE_NICKNAME)?;
    let nickname = req
        .nickname
        .as_deref()
        .map(clean_name)
        .filter(|n| !n.is_empty());
    sqlx::query("UPDATE guild_members SET nickname = $3 WHERE guild_id = $1 AND user_id = $2")
        .bind(guild_id)
        .bind(user.id)
        .bind(&nickname)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::GuildRefresh { guild_id },
    );
    let mut member = fetch_member(&st.db, guild_id, user.id).await?;
    member.online = st.is_online(member.id);
    Ok(Json(member))
}
