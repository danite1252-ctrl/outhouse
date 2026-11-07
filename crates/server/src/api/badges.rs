//! Profile badges: automatic ones (owner, admin, raid leader, perfect
//! attendance, founder, early member, bot) worked out on the fly, plus
//! custom badges an outhouse's admins make and hand out.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use outhouse_common::gateway::ServerEvent;
use outhouse_common::id::{option_as_string, vec_as_string};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::{AppState, Audience};

const MAX_BADGES_PER_GUILD: i64 = 30;
/// How many raids someone needs to have led for the Raid leader badge.
const RAID_LEADER_RAIDS: i64 = 3;
/// Perfect attendance: turned up to (at least) their last this-many raids.
const STREAK_RAIDS: i64 = 5;
/// The first this-many accounts on the server are Founders; the first
/// this-many people to join an outhouse are Early members.
const FOUNDERS: i64 = 10;
const EARLY_MEMBERS: i64 = 10;

#[derive(Debug, Serialize)]
pub struct BadgeInfo {
    /// "owner", "admin", … for automatic badges, the badge ID for custom ones.
    pub key: String,
    #[serde(with = "option_as_string")]
    pub id: Option<i64>,
    pub name: String,
    pub emoji: String,
    pub color: i32,
    pub description: String,
    pub auto: bool,
    #[serde(with = "vec_as_string")]
    pub holders: Vec<i64>,
}

fn auto_badge(
    key: &str,
    name: &str,
    emoji: &str,
    color: i32,
    description: &str,
    holders: Vec<i64>,
) -> BadgeInfo {
    BadgeInfo {
        key: key.into(),
        id: None,
        name: name.into(),
        emoji: emoji.into(),
        color,
        description: description.into(),
        auto: true,
        holders,
    }
}

#[derive(sqlx::FromRow)]
struct BadgeRow {
    id: i64,
    name: String,
    emoji: String,
    color: i32,
    description: String,
}

/// `GET /api/guilds/{id}/badges`: every badge in the outhouse and who has it.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Vec<BadgeInfo>>> {
    member_ctx(&st.db, guild_id, user.id).await?;
    let db = &st.db;
    let owner: Vec<i64> = sqlx::query_scalar("SELECT owner_id FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_all(db)
        .await?;
    let admins: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT mr.user_id FROM member_roles mr
         JOIN roles r ON r.id = mr.role_id
         JOIN guild_members m ON m.guild_id = mr.guild_id AND m.user_id = mr.user_id
         WHERE mr.guild_id = $1 AND (r.permissions & $2) <> 0
           AND mr.user_id <> (SELECT owner_id FROM guilds WHERE id = $1)",
    )
    .bind(guild_id)
    .bind(p::ADMINISTRATOR as i64)
    .fetch_all(db)
    .await?;
    let leaders: Vec<i64> = sqlx::query_scalar(
        "SELECT e.creator_id FROM events e
         JOIN guild_members m ON m.guild_id = e.guild_id AND m.user_id = e.creator_id
         WHERE e.guild_id = $1
         GROUP BY e.creator_id HAVING count(*) >= $2",
    )
    .bind(guild_id)
    .bind(RAID_LEADER_RAIDS)
    .fetch_all(db)
    .await?;
    let streaks: Vec<i64> = sqlx::query_scalar(
        "SELECT user_id FROM (
             SELECT a.user_id, a.status,
                    row_number() OVER (PARTITION BY a.user_id ORDER BY e.starts_at DESC) AS rn
             FROM event_attendance a
             JOIN events e ON e.id = a.event_id
             JOIN guild_members m ON m.guild_id = e.guild_id AND m.user_id = a.user_id
             WHERE e.guild_id = $1) recent
         WHERE rn <= $2
         GROUP BY user_id
         HAVING count(*) = $2 AND bool_and(status <> 'absent')",
    )
    .bind(guild_id)
    .bind(STREAK_RAIDS)
    .fetch_all(db)
    .await?;
    let founders: Vec<i64> = sqlx::query_scalar(
        "SELECT f.id FROM (SELECT id FROM users WHERE NOT is_bot
                           ORDER BY created_at, id LIMIT $2) f
         JOIN guild_members m ON m.user_id = f.id AND m.guild_id = $1",
    )
    .bind(guild_id)
    .bind(FOUNDERS)
    .fetch_all(db)
    .await?;
    let early: Vec<i64> = sqlx::query_scalar(
        "SELECT m.user_id FROM guild_members m JOIN users u ON u.id = m.user_id
         WHERE m.guild_id = $1 AND NOT u.is_bot
         ORDER BY m.joined_at, m.user_id LIMIT $2",
    )
    .bind(guild_id)
    .bind(EARLY_MEMBERS)
    .fetch_all(db)
    .await?;
    let bots: Vec<i64> = sqlx::query_scalar(
        "SELECT m.user_id FROM guild_members m JOIN users u ON u.id = m.user_id
         WHERE m.guild_id = $1 AND u.is_bot",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?;

    let mut out = vec![
        auto_badge(
            "owner",
            "Outhouse owner",
            "👑",
            0xF5C542,
            "Built this outhouse.",
            owner,
        ),
        auto_badge(
            "admin",
            "Admin",
            "🛠️",
            0xFF6A2B,
            "Has the Administrator permission here.",
            admins,
        ),
        auto_badge(
            "raid_leader",
            "Raid leader",
            "⚔️",
            0xC69B6D,
            "Has planned 3 or more raids here.",
            leaders,
        ),
        auto_badge(
            "attendance",
            "Perfect attendance",
            "🔥",
            0xFF4D4D,
            "Turned up to their last 5 raids in a row.",
            streaks,
        ),
        auto_badge(
            "founder",
            "Founder",
            "🏛️",
            0x8B7CF6,
            "One of the first 10 people on this Outhouse server.",
            founders,
        ),
        auto_badge(
            "early",
            "Early member",
            "🌱",
            0x4ADE80,
            "One of the first 10 people to join this outhouse.",
            early,
        ),
        auto_badge("bot", "Bot", "🤖", 0x5865F2, "A bot account.", bots),
    ];

    let custom = sqlx::query_as::<_, BadgeRow>(
        "SELECT id, name, emoji, color, description FROM badges
         WHERE guild_id = $1 ORDER BY created_at, id",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    let awards: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT a.badge_id, a.user_id FROM badge_awards a
         JOIN badges b ON b.id = a.badge_id
         JOIN guild_members m ON m.guild_id = b.guild_id AND m.user_id = a.user_id
         WHERE b.guild_id = $1 ORDER BY a.awarded_at",
    )
    .bind(guild_id)
    .fetch_all(db)
    .await?;
    for b in custom {
        out.push(BadgeInfo {
            key: b.id.to_string(),
            id: Some(b.id),
            name: b.name,
            emoji: b.emoji,
            color: b.color,
            description: b.description,
            auto: false,
            holders: awards
                .iter()
                .filter(|(badge, _)| *badge == b.id)
                .map(|(_, u)| *u)
                .collect(),
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct BadgeRequest {
    name: Option<String>,
    emoji: Option<String>,
    color: Option<i32>,
    description: Option<String>,
}

fn clean_name(name: &str) -> AppResult<String> {
    let name: String = name.trim().chars().take(32).collect();
    if name.is_empty() {
        return Err(AppError::BadRequest("Give the badge a name.".into()));
    }
    Ok(name)
}

fn clean_emoji(emoji: &str) -> AppResult<String> {
    let emoji = emoji.trim();
    if emoji.is_empty() || emoji.chars().count() > 8 || emoji.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest("Pick one emoji for the badge.".into()));
    }
    Ok(emoji.to_string())
}

fn changed(st: &AppState, guild_id: i64) {
    st.dispatch(
        Audience::Guild(guild_id),
        ServerEvent::BadgesChanged { guild_id },
    );
}

/// `POST /api/guilds/{id}/badges` (needs Manage Outhouse).
pub async fn create(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<BadgeRequest>,
) -> AppResult<Json<serde_json::Value>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let name = clean_name(req.name.as_deref().unwrap_or(""))?;
    let emoji = clean_emoji(req.emoji.as_deref().unwrap_or("🏅"))?;
    let description: String = req
        .description
        .as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(100)
        .collect();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM badges WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await?;
    if count >= MAX_BADGES_PER_GUILD {
        return Err(AppError::BadRequest(format!(
            "An outhouse can have up to {MAX_BADGES_PER_GUILD} badges."
        )));
    }
    let id = st.ids.next_id();
    let color = req.color.unwrap_or(0).clamp(0, 0xFF_FFFF);
    sqlx::query(
        "INSERT INTO badges (id, guild_id, name, emoji, color, description, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(guild_id)
    .bind(&name)
    .bind(&emoji)
    .bind(color)
    .bind(&description)
    .bind(user.id)
    .execute(&st.db)
    .await?;
    audit::Entry::new(guild_id, user.id, "badge_create")
        .target("badge", id, format!("{emoji} {name}"))
        .details(description.clone())
        .save(&st)
        .await;
    changed(&st, guild_id);
    Ok(Json(serde_json::json!({
        "id": id.to_string(), "key": id.to_string(), "name": name, "emoji": emoji,
        "color": color, "description": description, "auto": false, "holders": [],
    })))
}

/// Loads a custom badge and checks the caller may manage it.
async fn manageable(st: &AppState, badge_id: i64, user_id: i64) -> AppResult<(i64, BadgeRow)> {
    let row: Option<(i64, i64, String, String, i32, String)> = sqlx::query_as(
        "SELECT guild_id, id, name, emoji, color, description FROM badges WHERE id = $1",
    )
    .bind(badge_id)
    .fetch_optional(&st.db)
    .await?;
    let (guild_id, id, name, emoji, color, description) = row.ok_or(AppError::NotFound)?;
    member_ctx(&st.db, guild_id, user_id)
        .await?
        .require(p::MANAGE_GUILD)?;
    Ok((
        guild_id,
        BadgeRow {
            id,
            name,
            emoji,
            color,
            description,
        },
    ))
}

/// `PATCH /api/badges/{id}` (needs Manage Outhouse).
pub async fn update(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(badge_id): Path<i64>,
    Json(req): Json<BadgeRequest>,
) -> AppResult<StatusCode> {
    let (guild_id, old) = manageable(&st, badge_id, user.id).await?;
    let name = match &req.name {
        Some(n) => clean_name(n)?,
        None => old.name.clone(),
    };
    let emoji = match &req.emoji {
        Some(e) => clean_emoji(e)?,
        None => old.emoji.clone(),
    };
    let color = req
        .color
        .map(|c| c.clamp(0, 0xFF_FFFF))
        .unwrap_or(old.color);
    let description: String = match &req.description {
        Some(d) => d.trim().chars().take(100).collect(),
        None => old.description.clone(),
    };
    sqlx::query(
        "UPDATE badges SET name = $2, emoji = $3, color = $4, description = $5 WHERE id = $1",
    )
    .bind(badge_id)
    .bind(&name)
    .bind(&emoji)
    .bind(color)
    .bind(&description)
    .execute(&st.db)
    .await?;
    audit::Entry::new(guild_id, user.id, "badge_update")
        .target("badge", badge_id, format!("{emoji} {name}"))
        .details(audit::join([
            audit::change("Name", old.name.as_str(), name.as_str()),
            audit::change("Emoji", old.emoji.as_str(), emoji.as_str()),
            audit::change(
                "Colour",
                format!("#{:06x}", old.color),
                format!("#{color:06x}"),
            ),
            audit::change(
                "Description",
                old.description.as_str(),
                description.as_str(),
            ),
        ]))
        .save(&st)
        .await;
    changed(&st, guild_id);
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/badges/{id}` (needs Manage Outhouse).
pub async fn delete(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(badge_id): Path<i64>,
) -> AppResult<StatusCode> {
    let (guild_id, old) = manageable(&st, badge_id, user.id).await?;
    sqlx::query("DELETE FROM badges WHERE id = $1")
        .bind(badge_id)
        .execute(&st.db)
        .await?;
    audit::Entry::new(guild_id, user.id, "badge_delete")
        .target("badge", old.id, format!("{} {}", old.emoji, old.name))
        .save(&st)
        .await;
    changed(&st, guild_id);
    Ok(StatusCode::NO_CONTENT)
}

async fn member_name(st: &AppState, guild_id: i64, user_id: i64) -> AppResult<String> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(m.nickname, u.display_name) FROM guild_members m
         JOIN users u ON u.id = m.user_id WHERE m.guild_id = $1 AND m.user_id = $2",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?;
    name.ok_or(AppError::NotFound)
}

/// `PUT /api/badges/{id}/members/{user}`: gives someone the badge.
pub async fn award(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((badge_id, target)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let (guild_id, badge) = manageable(&st, badge_id, user.id).await?;
    let name = member_name(&st, guild_id, target).await?;
    let added = sqlx::query(
        "INSERT INTO badge_awards (badge_id, user_id, awarded_by) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(badge_id)
    .bind(target)
    .bind(user.id)
    .execute(&st.db)
    .await?
    .rows_affected();
    if added > 0 {
        audit::Entry::new(guild_id, user.id, "badge_award")
            .target("member", target, name)
            .details(format!("{} {}", badge.emoji, badge.name))
            .save(&st)
            .await;
        changed(&st, guild_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/badges/{id}/members/{user}`: takes the badge away.
pub async fn revoke(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((badge_id, target)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    let (guild_id, badge) = manageable(&st, badge_id, user.id).await?;
    let removed = sqlx::query("DELETE FROM badge_awards WHERE badge_id = $1 AND user_id = $2")
        .bind(badge_id)
        .bind(target)
        .execute(&st.db)
        .await?
        .rows_affected();
    if removed > 0 {
        let name = member_name(&st, guild_id, target)
            .await
            .unwrap_or_else(|_| "someone".into());
        audit::Entry::new(guild_id, user.id, "badge_revoke")
            .target("member", target, name)
            .details(format!("{} {}", badge.emoji, badge.name))
            .save(&st)
            .await;
        changed(&st, guild_id);
    }
    Ok(StatusCode::NO_CONTENT)
}
