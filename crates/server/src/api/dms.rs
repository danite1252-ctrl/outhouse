//! Direct messages: one-to-one and group conversations outside any
//! outhouse. A DM is a channel with no outhouse plus a list of the people
//! in it, so messages, uploads, reactions, edits and threads all reuse the
//! normal message endpoints. You can message anyone you share an outhouse with.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{
    channel_kind, AddDmRecipientRequest, CreateDmRequest, DmChannel, DmUser, UpdateDmRequest,
    MAX_GROUP_DM,
};
use sqlx::PgPool;

use crate::auth::{AuthUser, USER_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::perms::is_dm_participant;
use crate::state::{AppState, Audience};

const MAX_NAME_CHARS: usize = 100;

#[derive(sqlx::FromRow)]
struct DmRow {
    id: i64,
    kind: i16,
    name: String,
    owner_id: Option<i64>,
    last_message_id: Option<i64>,
    last_read_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct RecipientRow {
    channel_id: i64,
    id: i64,
    username: String,
    display_name: String,
    avatar_url: Option<String>,
    bot: bool,
}

/// Your DMs (or just one of them), most recently active first.
async fn load_dms(st: &AppState, user_id: i64, only: Option<i64>) -> AppResult<Vec<DmChannel>> {
    let rows = sqlx::query_as::<_, DmRow>(
        "SELECT c.id, c.kind, c.name, c.owner_id, c.last_message_id, rs.last_read_id
         FROM dm_participants p
         JOIN channels c ON c.id = p.channel_id
         LEFT JOIN read_states rs ON rs.channel_id = c.id AND rs.user_id = $1
         WHERE p.user_id = $1 AND ($2::BIGINT IS NULL OR c.id = $2)
         ORDER BY COALESCE(c.last_message_id, c.id) DESC",
    )
    .bind(user_id)
    .bind(only)
    .fetch_all(&st.db)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let sql = format!(
        "SELECT p.channel_id, {USER_COLUMNS}
         FROM dm_participants p JOIN users u ON u.id = p.user_id
         WHERE p.channel_id = ANY($1)
         ORDER BY p.joined_at, u.id"
    );
    let recipients = sqlx::query_as::<_, RecipientRow>(&sql)
        .bind(&ids)
        .fetch_all(&st.db)
        .await?;
    let hidden = super::friends::blocked_by(&st.db, user_id).await?;
    let mut by_channel: HashMap<i64, Vec<DmUser>> = HashMap::new();
    for r in recipients {
        by_channel.entry(r.channel_id).or_default().push(DmUser {
            online: !hidden.contains(&r.id) && st.is_online(r.id),
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            avatar_url: r.avatar_url,
            bot: r.bot,
        });
    }
    Ok(rows
        .into_iter()
        .map(|r| DmChannel {
            recipients: by_channel.remove(&r.id).unwrap_or_default(),
            id: r.id,
            kind: r.kind,
            name: r.name,
            owner_id: r.owner_id,
            last_message_id: r.last_message_id,
            last_read_id: r.last_read_id,
        })
        .collect())
}

async fn dm_for(st: &AppState, channel_id: i64, user_id: i64) -> AppResult<DmChannel> {
    load_dms(st, user_id, Some(channel_id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)
}

async fn participants(db: &PgPool, channel_id: i64) -> AppResult<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT user_id FROM dm_participants WHERE channel_id = $1")
            .bind(channel_id)
            .fetch_all(db)
            .await?,
    )
}

/// Sends everyone in the DM its new state.
async fn announce(st: &AppState, channel_id: i64) -> AppResult<()> {
    for user_id in participants(&st.db, channel_id).await? {
        if let Ok(dm) = dm_for(st, channel_id, user_id).await {
            st.dispatch(Audience::User(user_id), ServerEvent::DmUpdate(dm));
        }
    }
    Ok(())
}

/// Checks everyone in `ids` is a real person who shares an outhouse with you.
async fn check_reachable(db: &PgPool, me: i64, ids: &[i64]) -> AppResult<()> {
    let ok: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users u
         WHERE u.id = ANY($1) AND NOT u.is_bot
           AND (EXISTS (SELECT 1 FROM guild_members a
                        JOIN guild_members b ON a.guild_id = b.guild_id
                        WHERE a.user_id = u.id AND b.user_id = $2)
                OR EXISTS (SELECT 1 FROM friendships f
                           WHERE (f.user_a = $2 AND f.user_b = u.id)
                              OR (f.user_b = $2 AND f.user_a = u.id)))",
    )
    .bind(ids)
    .bind(me)
    .fetch_one(db)
    .await?;
    if ok as usize != ids.len() {
        return Err(AppError::BadRequest(
            "You can only message friends and people you share an outhouse with.".into(),
        ));
    }
    Ok(())
}

fn clean_name(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect()
}

/// Loads a DM you're in.
async fn my_dm(st: &AppState, channel_id: i64, user_id: i64) -> AppResult<DmRow> {
    if !is_dm_participant(&st.db, channel_id, user_id).await? {
        return Err(AppError::NotFound);
    }
    sqlx::query_as::<_, DmRow>(
        "SELECT id, kind, name, owner_id, last_message_id, NULL::BIGINT AS last_read_id
         FROM channels WHERE id = $1",
    )
    .bind(channel_id)
    .fetch_one(&st.db)
    .await
    .map_err(AppError::from)
}

/// `GET /api/dms`
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<DmChannel>>> {
    Ok(Json(load_dms(&st, user.id, None).await?))
}

/// `GET /api/users/@me/contacts`: people you can message (everyone you
/// share an outhouse with).
pub async fn contacts(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<DmUser>>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        username: String,
        display_name: String,
        avatar_url: Option<String>,
        bot: bool,
    }
    // Friends first, then everyone else you share an outhouse with; never
    // anyone either of you has blocked.
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM users u
         WHERE u.id <> $1 AND NOT u.is_bot
           AND (EXISTS (SELECT 1 FROM guild_members a
                        JOIN guild_members b ON a.guild_id = b.guild_id
                        WHERE a.user_id = u.id AND b.user_id = $1)
                OR EXISTS (SELECT 1 FROM friendships f
                           WHERE (f.user_a = $1 AND f.user_b = u.id)
                              OR (f.user_b = $1 AND f.user_a = u.id)))
           AND NOT EXISTS (SELECT 1 FROM user_blocks k
                           WHERE (k.user_id = $1 AND k.blocked_id = u.id)
                              OR (k.user_id = u.id AND k.blocked_id = $1))
         ORDER BY EXISTS (SELECT 1 FROM friendships f
                          WHERE (f.user_a = $1 AND f.user_b = u.id)
                             OR (f.user_b = $1 AND f.user_a = u.id)) DESC,
                  lower(u.display_name), u.id
         LIMIT 500"
    );
    let rows = sqlx::query_as::<_, Row>(&sql)
        .bind(user.id)
        .fetch_all(&st.db)
        .await?;
    let hidden = super::friends::blocked_by(&st.db, user.id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| DmUser {
                online: !hidden.contains(&r.id) && st.is_online(r.id),
                id: r.id,
                username: r.username,
                display_name: r.display_name,
                avatar_url: r.avatar_url,
                bot: r.bot,
            })
            .collect(),
    ))
}

/// `POST /api/dms` `{"user_ids": [...], "name"?}`: one person opens (or
/// reopens) your DM with them; two or more make a group DM.
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CreateDmRequest>,
) -> AppResult<Json<DmChannel>> {
    if user.bot {
        return Err(AppError::Forbidden);
    }
    let mut ids: Vec<i64> = req
        .user_ids
        .iter()
        .copied()
        .filter(|&id| id != user.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(AppError::BadRequest("Pick someone to message.".into()));
    }
    if ids.len() + 1 > MAX_GROUP_DM {
        return Err(AppError::BadRequest(format!(
            "Group DMs can have up to {MAX_GROUP_DM} people."
        )));
    }
    check_reachable(&st.db, user.id, &ids).await?;

    let channel_id = if ids.len() == 1 {
        let other = ids[0];
        let key = format!("{}:{}", user.id.min(other), user.id.max(other));
        // Privacy and blocks decide who can start a conversation; an
        // existing one can always be opened (sending is checked per message).
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE dm_key = $1")
            .bind(&key)
            .fetch_optional(&st.db)
            .await?;
        if existing.is_none() {
            super::friends::check_can_dm(&st.db, user.id, other).await?;
        }
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO channels (id, guild_id, name, kind, position, dm_key)
             VALUES ($1, NULL, '', $2, 0, $3)
             ON CONFLICT (dm_key) DO NOTHING
             RETURNING id",
        )
        .bind(st.ids.next_id())
        .bind(channel_kind::DM)
        .bind(&key)
        .fetch_optional(&st.db)
        .await?;
        match id {
            Some(id) => {
                sqlx::query(
                    "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2), ($1, $3)",
                )
                .bind(id)
                .bind(user.id)
                .bind(other)
                .execute(&st.db)
                .await?;
                announce(&st, id).await?;
                id
            }
            // Already exists: just open it.
            None => {
                sqlx::query_scalar("SELECT id FROM channels WHERE dm_key = $1")
                    .bind(&key)
                    .fetch_one(&st.db)
                    .await?
            }
        }
    } else {
        for &other in &ids {
            super::friends::check_can_dm(&st.db, user.id, other).await?;
        }
        let id = st.ids.next_id();
        let name = clean_name(req.name.as_deref().unwrap_or(""));
        let mut tx = st.db.begin().await?;
        sqlx::query(
            "INSERT INTO channels (id, guild_id, name, kind, position, owner_id)
             VALUES ($1, NULL, $2, $3, 0, $4)",
        )
        .bind(id)
        .bind(&name)
        .bind(channel_kind::GROUP_DM)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
        let mut everyone = ids.clone();
        everyone.insert(0, user.id);
        for (i, uid) in everyone.iter().enumerate() {
            // Stagger join times so the list keeps the order people were added.
            sqlx::query(
                "INSERT INTO dm_participants (channel_id, user_id, joined_at)
                 VALUES ($1, $2, now() + make_interval(secs => $3))",
            )
            .bind(id)
            .bind(uid)
            .bind(i as f64 / 1000.0)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        announce(&st, id).await?;
        id
    };
    Ok(Json(dm_for(&st, channel_id, user.id).await?))
}

/// `PATCH /api/dms/{id}` `{"name"}`: rename a group DM (anyone in it can).
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<UpdateDmRequest>,
) -> AppResult<Json<DmChannel>> {
    let dm = my_dm(&st, channel_id, user.id).await?;
    if dm.kind != channel_kind::GROUP_DM {
        return Err(AppError::BadRequest("Only group DMs have a name.".into()));
    }
    sqlx::query("UPDATE channels SET name = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(clean_name(&req.name))
        .execute(&st.db)
        .await?;
    announce(&st, channel_id).await?;
    Ok(Json(dm_for(&st, channel_id, user.id).await?))
}

/// `POST /api/dms/{id}/recipients` `{"user_id"}`: add someone to a group DM.
pub async fn add_recipient(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<AddDmRecipientRequest>,
) -> AppResult<Json<DmChannel>> {
    let dm = my_dm(&st, channel_id, user.id).await?;
    if dm.kind != channel_kind::GROUP_DM {
        return Err(AppError::BadRequest(
            "Start a new group DM to add people.".into(),
        ));
    }
    let current = participants(&st.db, channel_id).await?;
    if current.contains(&req.user_id) {
        return Ok(Json(dm_for(&st, channel_id, user.id).await?));
    }
    if current.len() >= MAX_GROUP_DM {
        return Err(AppError::BadRequest(format!(
            "Group DMs can have up to {MAX_GROUP_DM} people."
        )));
    }
    check_reachable(&st.db, user.id, &[req.user_id]).await?;
    super::friends::check_can_dm(&st.db, user.id, req.user_id).await?;
    sqlx::query(
        "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(channel_id)
    .bind(req.user_id)
    .execute(&st.db)
    .await?;
    announce(&st, channel_id).await?;
    Ok(Json(dm_for(&st, channel_id, user.id).await?))
}

/// `DELETE /api/dms/{id}/recipients/@me`: leave a group DM. The last one
/// out deletes it.
pub async fn leave(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
) -> AppResult<StatusCode> {
    let dm = my_dm(&st, channel_id, user.id).await?;
    if dm.kind != channel_kind::GROUP_DM {
        return Err(AppError::BadRequest(
            "You can't leave a one-to-one DM.".into(),
        ));
    }
    sqlx::query("DELETE FROM dm_participants WHERE channel_id = $1 AND user_id = $2")
        .bind(channel_id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    st.dispatch(
        Audience::User(user.id),
        ServerEvent::DmDelete { id: channel_id },
    );
    let left: Vec<i64> = sqlx::query_scalar(
        "SELECT user_id FROM dm_participants WHERE channel_id = $1 ORDER BY joined_at",
    )
    .bind(channel_id)
    .fetch_all(&st.db)
    .await?;
    if left.is_empty() {
        let keys: Vec<String> = sqlx::query_scalar(
            "SELECT a.storage_key FROM attachments a
             JOIN messages m ON m.id = a.message_id WHERE m.channel_id = $1",
        )
        .bind(channel_id)
        .fetch_all(&st.db)
        .await?;
        sqlx::query("DELETE FROM channels WHERE id = $1")
            .bind(channel_id)
            .execute(&st.db)
            .await?;
        crate::api::messages::remove_unused_files(&st, keys).await;
        return Ok(StatusCode::NO_CONTENT);
    }
    if dm.owner_id == Some(user.id) {
        sqlx::query("UPDATE channels SET owner_id = $2 WHERE id = $1")
            .bind(channel_id)
            .bind(left[0])
            .execute(&st.db)
            .await?;
    }
    announce(&st, channel_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Opens (or reopens) the one-to-one DM between two users without any
/// checks: used by the server itself, e.g. the helper bot messaging someone.
pub async fn open_direct(st: &AppState, a: i64, b: i64) -> AppResult<i64> {
    let key = format!("{}:{}", a.min(b), a.max(b));
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO channels (id, guild_id, name, kind, position, dm_key)
         VALUES ($1, NULL, '', $2, 0, $3)
         ON CONFLICT (dm_key) DO NOTHING
         RETURNING id",
    )
    .bind(st.ids.next_id())
    .bind(channel_kind::DM)
    .bind(&key)
    .fetch_optional(&st.db)
    .await?;
    match id {
        Some(id) => {
            sqlx::query(
                "INSERT INTO dm_participants (channel_id, user_id) VALUES ($1, $2), ($1, $3)
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(a)
            .bind(b)
            .execute(&st.db)
            .await?;
            announce(st, id).await?;
            Ok(id)
        }
        None => Ok(
            sqlx::query_scalar("SELECT id FROM channels WHERE dm_key = $1")
                .bind(&key)
                .fetch_one(&st.db)
                .await?,
        ),
    }
}
