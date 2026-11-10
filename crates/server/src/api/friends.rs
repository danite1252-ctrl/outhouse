//! Friends, friend requests, blocking and privacy settings.
//!
//! - A friend request goes to someone by username (or from their profile
//!   card); they accept or decline, you can cancel. Friends show on Home →
//!   Friends.
//! - Blocking someone ends any friendship or request between you. They
//!   can't DM you or send you friend requests, and they don't see when
//!   you're online. Their messages in shared channels are folded away (in
//!   the client).
//! - Privacy: who may start a DM with you (anyone in a shared outhouse, or
//!   friends only) and who may send you friend requests.

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{DmUser, PrivacySettings, Relationship, DM_GUILD};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::auth::{AuthUser, USER_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

/// Most friend requests you can have waiting on other people.
const MAX_OUTGOING: i64 = 100;

// ---- Checks used elsewhere ------------------------------------------------------

/// Has either of them blocked the other?
pub async fn blocked_either(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_blocks
                       WHERE (user_id = $1 AND blocked_id = $2)
                          OR (user_id = $2 AND blocked_id = $1))",
    )
    .bind(a)
    .bind(b)
    .fetch_one(db)
    .await?)
}

/// Has `user_id` blocked `viewer`? (Then `viewer` doesn't see them online.)
pub async fn has_blocked(db: &PgPool, user_id: i64, viewer: i64) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_blocks WHERE user_id = $1 AND blocked_id = $2)",
    )
    .bind(user_id)
    .bind(viewer)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

/// Everyone who has blocked `viewer` (their presence is hidden from them).
pub async fn blocked_by(db: &PgPool, viewer: i64) -> AppResult<HashSet<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM user_blocks WHERE blocked_id = $1")
        .bind(viewer)
        .fetch_all(db)
        .await?;
    Ok(ids.into_iter().collect())
}

pub async fn are_friends(db: &PgPool, a: i64, b: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM friendships WHERE user_a = $1 AND user_b = $2)",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(db)
    .await?)
}

/// May `me` start a new conversation with `other`? Refused when either has
/// blocked the other, or `other` only takes DMs from friends.
pub async fn check_can_dm(db: &PgPool, me: i64, other: i64) -> AppResult<()> {
    if blocked_either(db, me, other).await? {
        return Err(AppError::Refused(
            "You can't message this person right now.".into(),
        ));
    }
    // Teen protections (see api::safety).
    super::safety::check_teen_dm(db, me, other).await?;
    let privacy: Option<String> = sqlx::query_scalar("SELECT dm_privacy FROM users WHERE id = $1")
        .bind(other)
        .fetch_optional(db)
        .await?;
    if privacy.as_deref() == Some("friends") && !are_friends(db, me, other).await? {
        let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(other)
            .fetch_one(db)
            .await?;
        return Err(AppError::Refused(format!(
            "{name} only takes DMs from friends. Send them a friend request first."
        )));
    }
    Ok(())
}

// ---- Loading ----------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    bot: bool,
}

#[derive(sqlx::FromRow)]
struct RelRow {
    kind: String,
    since: DateTime<Utc>,
    id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    bot: bool,
}

fn dm_user(st: &AppState, r: UserRow, show_online: bool) -> DmUser {
    DmUser {
        online: show_online && st.is_online(r.id),
        id: r.id,
        username: r.username,
        display_name: r.display_name,
        avatar_url: r.avatar_url,
        bot: r.bot,
    }
}

/// Someone as `viewer` sees them (offline if they've blocked `viewer`).
async fn user_as_seen_by(st: &AppState, user_id: i64, viewer: i64) -> AppResult<DmUser> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users u WHERE u.id = $1");
    let row = sqlx::query_as::<_, UserRow>(&sql)
        .bind(user_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let hidden = has_blocked(&st.db, user_id, viewer).await;
    Ok(dm_user(st, row, !hidden))
}

/// Tells `viewer`'s clients where they stand with `other` now.
async fn announce(st: &AppState, viewer: i64, other: i64, kind: &str) -> AppResult<()> {
    let user = user_as_seen_by(st, other, viewer).await?;
    st.dispatch(
        Audience::User(viewer),
        ServerEvent::RelationshipUpdate {
            kind: kind.into(),
            user,
        },
    );
    Ok(())
}

/// `GET /api/users/@me/relationships`: friends, requests both ways, and
/// people you've blocked.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Relationship>>> {
    let sql = format!(
        "SELECT r.kind, r.since, {USER_COLUMNS}
         FROM (
             SELECT 'friend' AS kind, created_at AS since,
                    CASE WHEN user_a = $1 THEN user_b ELSE user_a END AS other
             FROM friendships WHERE user_a = $1 OR user_b = $1
             UNION ALL
             SELECT 'incoming', created_at, from_id FROM friend_requests WHERE to_id = $1
             UNION ALL
             SELECT 'outgoing', created_at, to_id FROM friend_requests WHERE from_id = $1
             UNION ALL
             SELECT 'blocked', created_at, blocked_id FROM user_blocks WHERE user_id = $1
         ) r
         JOIN users u ON u.id = r.other
         ORDER BY lower(u.display_name), u.id"
    );
    let rows = sqlx::query_as::<_, RelRow>(&sql)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    let hidden = blocked_by(&st.db, user.id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| Relationship {
                user: DmUser {
                    online: !hidden.contains(&r.id) && st.is_online(r.id),
                    id: r.id,
                    username: r.username,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                    bot: r.bot,
                },
                kind: r.kind,
                since: r.since,
            })
            .collect(),
    ))
}

// ---- Requests -----------------------------------------------------------------

#[derive(Deserialize)]
pub struct AddFriendRequest {
    username: String,
}

/// `POST /api/users/@me/relationships` `{"username"}`: sends a friend
/// request (or accepts theirs if they already asked you).
pub async fn add_by_username(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<AddFriendRequest>,
) -> AppResult<Json<Relationship>> {
    let name = req.username.trim().trim_start_matches('@').to_lowercase();
    if name.is_empty() {
        return Err(AppError::BadRequest("Type their username.".into()));
    }
    let id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE lower(username) = $1 AND NOT is_bot")
            .bind(&name)
            .fetch_optional(&st.db)
            .await?;
    let id = id.ok_or_else(|| {
        AppError::BadRequest(
            "Nobody goes by that username. Check the spelling (it's their username, not their display name)."
                .into(),
        )
    })?;
    Ok(Json(befriend(&st, user.id, user.bot, id).await?))
}

/// `PUT /api/users/@me/relationships/{id}`: sends a friend request to
/// someone, or accepts theirs.
pub async fn add_by_id(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(other): Path<i64>,
) -> AppResult<Json<Relationship>> {
    Ok(Json(befriend(&st, user.id, user.bot, other).await?))
}

async fn relationship(st: &AppState, me: i64, other: i64, kind: &str) -> AppResult<Relationship> {
    Ok(Relationship {
        kind: kind.into(),
        user: user_as_seen_by(st, other, me).await?,
        since: Utc::now(),
    })
}

async fn befriend(st: &AppState, me: i64, me_bot: bool, other: i64) -> AppResult<Relationship> {
    if me_bot {
        return Err(AppError::Forbidden);
    }
    if me == other {
        return Err(AppError::BadRequest(
            "You can't be your own friend (well, not here).".into(),
        ));
    }
    let target: Option<(bool, String)> =
        sqlx::query_as("SELECT is_bot, friend_request_privacy FROM users WHERE id = $1")
            .bind(other)
            .fetch_optional(&st.db)
            .await?;
    let Some((is_bot, privacy)) = target else {
        return Err(AppError::NotFound);
    };
    if is_bot {
        return Err(AppError::BadRequest("Bots don't do friends.".into()));
    }
    let i_blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_blocks WHERE user_id = $1 AND blocked_id = $2)",
    )
    .bind(me)
    .bind(other)
    .fetch_one(&st.db)
    .await?;
    if i_blocked {
        return Err(AppError::BadRequest(
            "You've blocked them. Unblock them first.".into(),
        ));
    }
    // Being blocked looks the same as not taking requests.
    let refused =
        || AppError::Refused("They aren't taking friend requests from you right now.".into());
    if has_blocked(&st.db, other, me).await {
        return Err(refused());
    }
    if are_friends(&st.db, me, other).await? {
        return Err(AppError::Conflict("You're already friends.".into()));
    }

    // They already asked you: that's a yes.
    let theirs = sqlx::query("DELETE FROM friend_requests WHERE from_id = $1 AND to_id = $2")
        .bind(other)
        .bind(me)
        .execute(&st.db)
        .await?
        .rows_affected();
    if theirs > 0 {
        sqlx::query(
            "INSERT INTO friendships (user_a, user_b) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(me.min(other))
        .bind(me.max(other))
        .execute(&st.db)
        .await?;
        announce(st, me, other, "friend").await?;
        announce(st, other, me, "friend").await?;
        return relationship(st, me, other, "friend").await;
    }

    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM friend_requests WHERE from_id = $1 AND to_id = $2)",
    )
    .bind(me)
    .bind(other)
    .fetch_one(&st.db)
    .await?;
    if already {
        return relationship(st, me, other, "outgoing").await;
    }

    // Teens only get requests from people they share an outhouse with who
    // are friends of friends or in a family outhouse (api::safety).
    super::safety::check_teen_friend_request(&st.db, me, other).await?;
    let allowed = match privacy.as_str() {
        "nobody" => false,
        "shared" => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM guild_members a
                           JOIN guild_members b ON a.guild_id = b.guild_id
                           WHERE a.user_id = $1 AND b.user_id = $2)",
            )
            .bind(me)
            .bind(other)
            .fetch_one(&st.db)
            .await?
        }
        "friends_of_friends" => {
            sqlx::query_scalar(
                "WITH mine AS (
                 SELECT CASE WHEN user_a = $1 THEN user_b ELSE user_a END AS f
                 FROM friendships WHERE user_a = $1 OR user_b = $1),
             theirs AS (
                 SELECT CASE WHEN user_a = $2 THEN user_b ELSE user_a END AS f
                 FROM friendships WHERE user_a = $2 OR user_b = $2)
             SELECT EXISTS(SELECT 1 FROM mine JOIN theirs USING (f))",
            )
            .bind(me)
            .bind(other)
            .fetch_one(&st.db)
            .await?
        }
        _ => true,
    };
    if !allowed {
        return Err(refused());
    }
    let waiting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM friend_requests WHERE from_id = $1")
            .bind(me)
            .fetch_one(&st.db)
            .await?;
    if waiting >= MAX_OUTGOING {
        return Err(AppError::BadRequest(format!(
            "You have {MAX_OUTGOING} friend requests waiting. Cancel some first."
        )));
    }
    sqlx::query(
        "INSERT INTO friend_requests (from_id, to_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(me)
    .bind(other)
    .execute(&st.db)
    .await?;
    announce(st, me, other, "outgoing").await?;
    announce(st, other, me, "incoming").await?;
    spawn_request_push(st, other, me);
    relationship(st, me, other, "outgoing").await
}

/// Push notification about a new friend request (if they get DM
/// notifications and aren't looking at Outhouse).
fn spawn_request_push(st: &AppState, to: i64, from: i64) {
    if st.vapid.is_none() || !st.is_away(to) {
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        let level: Option<String> = sqlx::query_scalar(
            "SELECT level FROM notify_levels WHERE user_id = $1 AND guild_id = $2",
        )
        .bind(to)
        .bind(DM_GUILD)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
        if level.as_deref() == Some("nothing") {
            return;
        }
        let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(from)
            .fetch_one(&st.db)
            .await
            .unwrap_or_else(|_| "Someone".into());
        let payload = json!({
            "title": "New friend request",
            "body": format!("{name} wants to be friends."),
            "kind": "friend",
            "guild_id": "0",
            "channel_id": "friends",
            "tag": format!("friend-{from}"),
            "url": "/app?friends=1",
        });
        if let Err(e) = crate::push::send_to_user(&st, to, &payload).await {
            tracing::warn!(error = %e, "friend request push failed");
        }
    });
}

/// `DELETE /api/users/@me/relationships/{id}`: unfriend, decline their
/// request, or cancel yours.
pub async fn remove(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(other): Path<i64>,
) -> AppResult<StatusCode> {
    let me = user.id;
    let unfriended = sqlx::query("DELETE FROM friendships WHERE user_a = $1 AND user_b = $2")
        .bind(me.min(other))
        .bind(me.max(other))
        .execute(&st.db)
        .await?
        .rows_affected();
    let requests = sqlx::query(
        "DELETE FROM friend_requests
         WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)",
    )
    .bind(me)
    .bind(other)
    .execute(&st.db)
    .await?
    .rows_affected();
    if unfriended + requests > 0 {
        announce(&st, me, other, "none").await?;
        announce(&st, other, me, "none").await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Blocking -----------------------------------------------------------------

/// `PUT /api/users/@me/blocks/{id}`
pub async fn block(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(other): Path<i64>,
) -> AppResult<Json<Relationship>> {
    let me = user.id;
    if me == other {
        return Err(AppError::BadRequest("You can't block yourself.".into()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(other)
        .fetch_one(&st.db)
        .await?;
    if !exists {
        return Err(AppError::NotFound);
    }
    let mut tx = st.db.begin().await?;
    let unfriended = sqlx::query("DELETE FROM friendships WHERE user_a = $1 AND user_b = $2")
        .bind(me.min(other))
        .bind(me.max(other))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let requests = sqlx::query(
        "DELETE FROM friend_requests
         WHERE (from_id = $1 AND to_id = $2) OR (from_id = $2 AND to_id = $1)",
    )
    .bind(me)
    .bind(other)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query(
        "INSERT INTO user_blocks (user_id, blocked_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(me)
    .bind(other)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    announce(&st, me, other, "blocked").await?;
    if unfriended + requests > 0 {
        announce(&st, other, me, "none").await?;
    }
    // They stop seeing you online.
    if st.is_online(me) {
        st.dispatch(
            Audience::User(other),
            ServerEvent::PresenceUpdate {
                user_id: me,
                online: false,
            },
        );
    }
    Ok(Json(relationship(&st, me, other, "blocked").await?))
}

/// `DELETE /api/users/@me/blocks/{id}`
pub async fn unblock(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(other): Path<i64>,
) -> AppResult<StatusCode> {
    let me = user.id;
    let removed = sqlx::query("DELETE FROM user_blocks WHERE user_id = $1 AND blocked_id = $2")
        .bind(me)
        .bind(other)
        .execute(&st.db)
        .await?
        .rows_affected();
    if removed > 0 {
        announce(&st, me, other, "none").await?;
        if st.is_online(me) && !has_blocked(&st.db, other, me).await {
            st.dispatch(
                Audience::User(other),
                ServerEvent::PresenceUpdate {
                    user_id: me,
                    online: true,
                },
            );
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Privacy --------------------------------------------------------------------

/// `GET /api/users/@me/privacy`
pub async fn get_privacy(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<PrivacySettings>> {
    let (allow_dms, allow_friend_requests): (String, String) =
        sqlx::query_as("SELECT dm_privacy, friend_request_privacy FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    Ok(Json(PrivacySettings {
        allow_dms,
        allow_friend_requests,
    }))
}

#[derive(Deserialize)]
pub struct PrivacyUpdate {
    allow_dms: Option<String>,
    allow_friend_requests: Option<String>,
}

/// `PUT /api/users/@me/privacy` `{"allow_dms"?, "allow_friend_requests"?}`
pub async fn set_privacy(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<PrivacyUpdate>,
) -> AppResult<Json<PrivacySettings>> {
    if let Some(v) = &req.allow_dms {
        if !matches!(v.as_str(), "everyone" | "friends") {
            return Err(AppError::BadRequest(
                "allow_dms is \"everyone\" or \"friends\".".into(),
            ));
        }
    }
    if let Some(v) = &req.allow_friend_requests {
        if !matches!(
            v.as_str(),
            "everyone" | "shared" | "friends_of_friends" | "nobody"
        ) {
            return Err(AppError::BadRequest(
                "allow_friend_requests is \"everyone\", \"shared\", \"friends_of_friends\" or \"nobody\"."
                    .into(),
            ));
        }
    }
    let (allow_dms, allow_friend_requests): (String, String) = sqlx::query_as(
        "UPDATE users SET dm_privacy = COALESCE($2, dm_privacy),
                friend_request_privacy = COALESCE($3, friend_request_privacy)
         WHERE id = $1 RETURNING dm_privacy, friend_request_privacy",
    )
    .bind(user.id)
    .bind(&req.allow_dms)
    .bind(&req.allow_friend_requests)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(PrivacySettings {
        allow_dms,
        allow_friend_requests,
    }))
}
