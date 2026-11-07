//! Group finder ("LFG"): a post looking for people for a dungeon, raid or
//! anything else, with tank / healer / DPS slots. People join from the card
//! in chat, the leader can remove people or close it, it shows as full when
//! every slot is taken, and it expires after a while. Changes go out live
//! (the message is sent again).

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use outhouse_common::models::{channel_kind, Message};
use outhouse_common::permissions as p;
use outhouse_common::wow::{
    class_name, find_activity, game_id, max_level_for, CreateLfgRequest, JoinLfgRequest, LfgGroup,
    LfgMember, TOP_LEVEL,
};
use sqlx::PgPool;

use super::messages::fetch_message;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::{channel_access, channel_audience, fetch_channel, ChannelAccess};
use crate::state::AppState;

const MAX_ACTIVITY_CHARS: usize = 80;
const MAX_NOTE_CHARS: usize = 300;
/// "First Last" in WoW: Forever: two names of up to 12 letters.
const MAX_CHARACTER_CHARS: usize = 25;
const MAX_SLOTS: i32 = 40;
const DEFAULT_EXPIRY_MINUTES: i32 = 120;
/// Two days.
const MAX_EXPIRY_MINUTES: i32 = 2 * 24 * 60;

fn clean(raw: &str, max: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

#[derive(sqlx::FromRow)]
struct GroupRow {
    message_id: i64,
    channel_id: i64,
    leader_id: i64,
    activity: String,
    activity_kind: String,
    min_level: Option<i32>,
    max_level: Option<i32>,
    max_tanks: i32,
    max_healers: i32,
    max_dps: i32,
    starts_at: Option<DateTime<Utc>>,
    note: String,
    game: String,
    closed: bool,
    expired: bool,
    expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    message_id: i64,
    user_id: i64,
    display_name: String,
    role: String,
    class: String,
    character: String,
    level: Option<i32>,
    joined_at: DateTime<Utc>,
}

fn role_max(g: &GroupRow, role: &str) -> i32 {
    match role {
        "tank" => g.max_tanks,
        "healer" => g.max_healers,
        "dps" => g.max_dps,
        _ => 0,
    }
}

/// Groups for these messages (for showing them in chat).
pub async fn groups_for_messages(db: &PgPool, ids: &[i64]) -> AppResult<HashMap<i64, LfgGroup>> {
    let groups = sqlx::query_as::<_, GroupRow>(
        "SELECT g.message_id, m.channel_id, g.leader_id, g.activity, g.activity_kind,
                g.min_level, g.max_level, g.max_tanks, g.max_healers, g.max_dps, g.starts_at,
                g.note, g.game, g.closed, g.expired, g.expires_at
         FROM lfg_groups g JOIN messages m ON m.id = g.message_id
         WHERE g.message_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    if groups.is_empty() {
        return Ok(HashMap::new());
    }
    let group_ids: Vec<i64> = groups.iter().map(|g| g.message_id).collect();
    let members = sqlx::query_as::<_, MemberRow>(
        "SELECT lm.message_id, lm.user_id,
                COALESCE(gm.nickname, u.display_name) AS display_name,
                lm.role, lm.class, lm.character, lm.level, lm.joined_at
         FROM lfg_members lm
         JOIN users u ON u.id = lm.user_id
         JOIN messages m ON m.id = lm.message_id
         JOIN channels ch ON ch.id = m.channel_id
         LEFT JOIN guild_members gm ON gm.guild_id = ch.guild_id AND gm.user_id = lm.user_id
         WHERE lm.message_id = ANY($1)
         ORDER BY lm.joined_at, lm.user_id",
    )
    .bind(&group_ids[..])
    .fetch_all(db)
    .await?;
    let mut by_group: HashMap<i64, Vec<LfgMember>> = HashMap::new();
    for m in members {
        by_group.entry(m.message_id).or_default().push(LfgMember {
            user_id: m.user_id,
            display_name: m.display_name,
            role: m.role,
            class: m.class,
            character: m.character,
            level: m.level,
            joined_at: m.joined_at,
        });
    }
    Ok(groups
        .into_iter()
        .map(|g| {
            let members = by_group.remove(&g.message_id).unwrap_or_default();
            let count = |role: &str| members.iter().filter(|m| m.role == role).count() as i32;
            let full = g.max_tanks + g.max_healers + g.max_dps > 0
                && ["tank", "healer", "dps"]
                    .iter()
                    .all(|r| count(r) >= role_max(&g, r));
            (
                g.message_id,
                LfgGroup {
                    message_id: g.message_id,
                    channel_id: g.channel_id,
                    leader_id: g.leader_id,
                    activity: g.activity,
                    activity_kind: g.activity_kind,
                    min_level: g.min_level,
                    max_level: g.max_level,
                    max_tanks: g.max_tanks,
                    max_healers: g.max_healers,
                    max_dps: g.max_dps,
                    starts_at: g.starts_at,
                    note: g.note,
                    game: g.game,
                    closed: g.closed,
                    expired: g.expired || g.expires_at <= Utc::now(),
                    expires_at: g.expires_at,
                    full,
                    members,
                },
            )
        })
        .collect())
}

struct CleanJoin {
    role: String,
    class: String,
    character: String,
    level: Option<i32>,
}

fn clean_join(req: &JoinLfgRequest) -> AppResult<CleanJoin> {
    let role = req.role.trim().to_ascii_lowercase();
    if !matches!(role.as_str(), "tank" | "healer" | "dps") {
        return Err(AppError::BadRequest(
            "Join as a tank, healer or DPS.".into(),
        ));
    }
    let class = if req.class.trim().is_empty() {
        String::new()
    } else {
        class_name(&req.class)
            .ok_or_else(|| AppError::BadRequest("That's not a class in WoW: Forever.".into()))?
            .to_string()
    };
    if let Some(level) = req.level {
        if !(1..=TOP_LEVEL).contains(&level) {
            return Err(AppError::BadRequest(format!(
                "Levels go from 1 to {TOP_LEVEL}."
            )));
        }
    }
    Ok(CleanJoin {
        role,
        class,
        character: clean(&req.character, MAX_CHARACTER_CHARS),
        level: req.level,
    })
}

async fn load_group(st: &AppState, channel_id: i64, message_id: i64) -> AppResult<LfgGroup> {
    let group = groups_for_messages(&st.db, &[message_id])
        .await?
        .remove(&message_id)
        .ok_or(AppError::NotFound)?;
    if group.channel_id != channel_id {
        return Err(AppError::NotFound);
    }
    Ok(group)
}

async fn announce(st: &AppState, access: &ChannelAccess, message_id: i64) -> AppResult<Message> {
    let message = fetch_message(&st.db, message_id).await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::MessageUpdate(message.clone()),
    );
    Ok(message)
}

/// `POST /api/channels/{id}/lfg`
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(channel_id): Path<i64>,
    Json(req): Json<CreateLfgRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    if access.channel.kind == channel_kind::VOICE {
        return Err(AppError::BadRequest(
            "Group posts go in text channels.".into(),
        ));
    }
    let activity = clean(&req.activity, MAX_ACTIVITY_CHARS);
    if activity.is_empty() {
        return Err(AppError::BadRequest(
            "Say what the group is for (a dungeon, raid or anything else).".into(),
        ));
    }
    let listed = find_activity(&activity);
    let (activity, kind) = match &listed {
        Some(a) => (a.name.clone(), a.kind.clone()),
        None => (activity, "other".to_string()),
    };
    let (def_t, def_h, def_d) =
        outhouse_common::wow::default_roles(listed.as_ref().map(|a| a.size).unwrap_or(5));
    let max_tanks = req.max_tanks.unwrap_or(def_t);
    let max_healers = req.max_healers.unwrap_or(def_h);
    let max_dps = req.max_dps.unwrap_or(def_d);
    if [max_tanks, max_healers, max_dps]
        .iter()
        .any(|n| !(0..=MAX_SLOTS).contains(n))
        || max_tanks + max_healers + max_dps < 1
        || max_tanks + max_healers + max_dps > MAX_SLOTS
    {
        return Err(AppError::BadRequest(format!(
            "A group needs 1 to {MAX_SLOTS} spots in total."
        )));
    }
    // Which game it's for ("" = any).
    let game = if req.game.trim().is_empty() {
        String::new()
    } else {
        game_id(&req.game)
            .ok_or_else(|| AppError::BadRequest("Unknown game.".into()))?
            .to_string()
    };
    let top = if game.is_empty() {
        TOP_LEVEL
    } else {
        max_level_for(&game)
    };
    let min_level = req.min_level.or(listed.as_ref().map(|a| a.min_level));
    let max_level = req.max_level.or(listed.as_ref().map(|a| a.max_level));
    for level in [min_level, max_level].into_iter().flatten() {
        if !(1..=top).contains(&level) {
            return Err(AppError::BadRequest(format!("Levels go from 1 to {top}.")));
        }
    }
    if let (Some(lo), Some(hi)) = (min_level, max_level) {
        if lo > hi {
            return Err(AppError::BadRequest(
                "The lowest level is above the highest.".into(),
            ));
        }
    }
    let now = Utc::now();
    if let Some(start) = req.starts_at {
        if start < now - chrono::Duration::hours(1) || start > now + chrono::Duration::days(14) {
            return Err(AppError::BadRequest(
                "Pick a start time in the next two weeks.".into(),
            ));
        }
    }
    let expiry = req.expires_minutes.unwrap_or(DEFAULT_EXPIRY_MINUTES);
    if !(1..=MAX_EXPIRY_MINUTES).contains(&expiry) {
        return Err(AppError::BadRequest(
            "Group posts can stay up for up to two days.".into(),
        ));
    }
    let expires_at = match req.starts_at {
        // Keep it up until a couple of hours after the start.
        Some(start) if req.expires_minutes.is_none() => {
            (start + chrono::Duration::hours(2)).max(now + chrono::Duration::minutes(30))
        }
        _ => now + chrono::Duration::minutes(expiry as i64),
    };
    let note = clean(&req.note, MAX_NOTE_CHARS);
    let leader = req.leader.as_ref().map(clean_join).transpose()?;
    if let Some(l) = &leader {
        let max = match l.role.as_str() {
            "tank" => max_tanks,
            "healer" => max_healers,
            _ => max_dps,
        };
        if max == 0 {
            return Err(AppError::BadRequest(
                "Your own role needs at least one spot.".into(),
            ));
        }
    }

    let id = st.ids.next_id();
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, channel_id, author_id, content) VALUES ($1, $2, $3, '')",
    )
    .bind(id)
    .bind(channel_id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO lfg_groups (message_id, leader_id, activity, activity_kind, min_level,
             max_level, max_tanks, max_healers, max_dps, starts_at, note, expires_at, game)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&activity)
    .bind(&kind)
    .bind(min_level)
    .bind(max_level)
    .bind(max_tanks)
    .bind(max_healers)
    .bind(max_dps)
    .bind(req.starts_at)
    .bind(&note)
    .bind(expires_at)
    .bind(&game)
    .execute(&mut *tx)
    .await?;
    if let Some(l) = &leader {
        sqlx::query(
            "INSERT INTO lfg_members (message_id, user_id, role, class, character, level)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(user.id)
        .bind(&l.role)
        .bind(&l.class)
        .bind(&l.character)
        .bind(l.level)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE channels SET last_message_id = $2 WHERE id = $1")
        .bind(channel_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO read_states (user_id, channel_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_id = EXCLUDED.last_read_id",
    )
    .bind(user.id)
    .bind(channel_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let message = fetch_message(&st.db, id).await?;
    st.dispatch(
        channel_audience(&access.channel),
        ServerEvent::MessageCreate(message.clone()),
    );
    Ok(Json(message))
}

/// `PUT /api/channels/{id}/lfg/{message}/join`: join (or change your role).
pub async fn join(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<JoinLfgRequest>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    access.require(p::SEND_MESSAGES)?;
    let group = load_group(&st, channel_id, message_id).await?;
    if group.closed || group.expired {
        return Err(AppError::BadRequest("This group has closed.".into()));
    }
    let j = clean_join(&req)?;
    let max = match j.role.as_str() {
        "tank" => group.max_tanks,
        "healer" => group.max_healers,
        _ => group.max_dps,
    };
    let taken = group
        .members
        .iter()
        .filter(|m| m.role == j.role && m.user_id != user.id)
        .count() as i32;
    if taken >= max {
        let label = match j.role.as_str() {
            "tank" => "tanks",
            "healer" => "healers",
            _ => "DPS",
        };
        return Err(AppError::BadRequest(if max == 0 {
            format!("This group isn't after {label}.")
        } else {
            format!("They've got all the {label} they need.")
        }));
    }
    sqlx::query(
        "INSERT INTO lfg_members (message_id, user_id, role, class, character, level)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (message_id, user_id) DO UPDATE
         SET role = EXCLUDED.role, class = EXCLUDED.class, character = EXCLUDED.character,
             level = EXCLUDED.level",
    )
    .bind(message_id)
    .bind(user.id)
    .bind(&j.role)
    .bind(&j.class)
    .bind(&j.character)
    .bind(j.level)
    .execute(&st.db)
    .await?;
    Ok(Json(announce(&st, &access, message_id).await?))
}

/// `DELETE /api/channels/{id}/lfg/{message}/members/{user}`: leave, or (the
/// leader and moderators) remove someone.
pub async fn remove_member(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id, member_id)): Path<(i64, i64, i64)>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let group = load_group(&st, channel_id, message_id).await?;
    if member_id != user.id && group.leader_id != user.id {
        access.require(p::MANAGE_MESSAGES)?;
    }
    sqlx::query("DELETE FROM lfg_members WHERE message_id = $1 AND user_id = $2")
        .bind(message_id)
        .bind(member_id)
        .execute(&st.db)
        .await?;
    Ok(Json(announce(&st, &access, message_id).await?))
}

/// `POST /api/channels/{id}/lfg/{message}/close`: the leader (or a moderator)
/// stops people joining.
pub async fn close(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Json<Message>> {
    let access = channel_access(&st.db, channel_id, user.id).await?;
    let group = load_group(&st, channel_id, message_id).await?;
    if group.leader_id != user.id {
        access.require(p::MANAGE_MESSAGES)?;
    }
    sqlx::query("UPDATE lfg_groups SET closed = true WHERE message_id = $1")
        .bind(message_id)
        .execute(&st.db)
        .await?;
    Ok(Json(announce(&st, &access, message_id).await?))
}

/// Marks groups whose time is up, every 15 seconds.
pub fn spawn_expirer(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            if let Err(e) = expire_due(&st).await {
                tracing::warn!(error = %e, "expiring group posts failed");
            }
        }
    });
}

async fn expire_due(st: &AppState) -> AppResult<()> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "UPDATE lfg_groups SET expired = true
         WHERE NOT expired AND expires_at <= now()
         RETURNING message_id",
    )
    .fetch_all(&st.db)
    .await?;
    for id in ids {
        let message = fetch_message(&st.db, id).await?;
        let channel = fetch_channel(&st.db, message.channel_id).await?;
        st.dispatch(
            channel_audience(&channel),
            ServerEvent::MessageUpdate(message),
        );
    }
    Ok(())
}
