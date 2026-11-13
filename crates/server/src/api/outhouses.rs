//! The site owner's Outhouses directory (User settings → Outhouses): every
//! outhouse on the server with its numbers, a read-only look inside for
//! moderation (without joining; every look is in the safety log), family
//! outhouses, and shutting one down (frozen, hidden from members, data kept
//! as evidence), restoring it, or deleting it for good.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::gateway::ServerEvent;
use serde::Deserialize;
use serde_json::{json, Value};

use super::safety::{guild_log, log_for_guild, require_owner};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Audience};

pub const PAGE_SIZE: i64 = 25;
const MAX_REASON: usize = 500;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub sort: String,
    /// 1-based.
    #[serde(default)]
    pub page: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    name: String,
    icon_key: Option<String>,
    owner_id: i64,
    owner_username: String,
    owner_name: String,
    owner_suspended: bool,
    created_at: DateTime<Utc>,
    family: bool,
    detector_enabled: bool,
    shut_down_at: Option<DateTime<Utc>>,
    shut_down_reason: String,
    members_remove_at: Option<DateTime<Utc>>,
    members_removed_at: Option<DateTime<Utc>>,
    member_snapshot: Option<Value>,
    members: i64,
    channels: i64,
    messages_7d: i64,
    messages_30d: i64,
    open_reports: i64,
    previous_names: Vec<String>,
    total: i64,
}

const ROW_SQL: &str = "
    SELECT g.id, g.name, g.icon_key, g.owner_id, u.username AS owner_username,
           u.display_name AS owner_name, COALESCE(u.suspended_until > now(), false) AS owner_suspended,
           g.created_at, g.family, g.detector_enabled, g.shut_down_at, g.shut_down_reason,
           g.members_remove_at, g.members_removed_at, g.member_snapshot,
           (SELECT count(*) FROM guild_members m WHERE m.guild_id = g.id) AS members,
           (SELECT count(*) FROM channels c WHERE c.guild_id = g.id) AS channels,
           (SELECT count(*) FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE c.guild_id = g.id AND m.created_at > now() - interval '7 days') AS messages_7d,
           (SELECT count(*) FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE c.guild_id = g.id AND m.created_at > now() - interval '30 days') AS messages_30d,
           (SELECT count(*) FROM reports r WHERE r.guild_id = g.id AND r.status <> 'closed') AS open_reports,
           ARRAY(SELECT h.old_name FROM guild_name_history h WHERE h.guild_id = g.id
                 ORDER BY h.changed_at DESC LIMIT 5) AS previous_names,
           count(*) OVER () AS total
    FROM guilds g JOIN users u ON u.id = g.owner_id
    WHERE g.id <> 0";

/// `%` and `_` typed in the search box are just characters.
fn like_pattern(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn sort_sql(sort: &str) -> &'static str {
    match sort {
        "members" => "members DESC, lower(g.name), g.id",
        "created" => "g.created_at DESC, g.id DESC",
        "activity" => "messages_7d DESC, messages_30d DESC, lower(g.name), g.id",
        "reports" => "open_reports DESC, lower(g.name), g.id",
        _ => "lower(g.name), g.id",
    }
}

fn row_json(st: &AppState, r: &Row, online: usize) -> Value {
    json!({
        "id": r.id.to_string(),
        "name": r.name,
        "icon_url": r.icon_key.as_ref().map(|k| format!("/files/{k}/icon")),
        "owner": {
            "id": r.owner_id.to_string(),
            "username": r.owner_username,
            "display_name": r.owner_name,
            "suspended": r.owner_suspended,
            "online": st.is_online(r.owner_id),
        },
        "created_at": r.created_at,
        "members": r.members,
        "online": online,
        "channels": r.channels,
        "messages_7d": r.messages_7d,
        "messages_30d": r.messages_30d,
        "open_reports": r.open_reports,
        "family": r.family,
        "detector_enabled": r.detector_enabled,
        "shut_down": r.shut_down_at.is_some(),
        "shut_down_at": r.shut_down_at,
        "shut_down_reason": r.shut_down_reason,
        "members_remove_at": r.members_remove_at,
        "members_removed_at": r.members_removed_at,
        "removed_members": r.member_snapshot.as_ref().and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0),
        "previous_names": r.previous_names,
    })
}

async fn online_in(st: &AppState, guild_id: i64) -> AppResult<usize> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM guild_members WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    Ok(ids.iter().filter(|&&id| st.is_online(id)).count())
}

/// `GET /api/safety/outhouses?q=&sort=name|members|created|activity|reports&page=`
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let term = q.q.trim().chars().take(100).collect::<String>();
    let page = q.page.unwrap_or(1).clamp(1, 100_000);
    let sql = format!(
        "{ROW_SQL}
           AND ($1 = '' OR g.name ILIKE $2 OR u.username ILIKE $2 OR g.id::TEXT = $1
                OR EXISTS (SELECT 1 FROM guild_name_history h WHERE h.guild_id = g.id AND h.old_name ILIKE $2))
         ORDER BY {}
         LIMIT $3 OFFSET $4",
        sort_sql(&q.sort)
    );
    let rows = sqlx::query_as::<_, Row>(&sql)
        .bind(&term)
        .bind(like_pattern(&term))
        .bind(PAGE_SIZE)
        .bind((page - 1) * PAGE_SIZE)
        .fetch_all(&st.db)
        .await?;
    let matching = rows.first().map(|r| r.total).unwrap_or(0);
    let mut items = Vec::new();
    for r in &rows {
        items.push(row_json(&st, r, online_in(&st, r.id).await?));
    }
    let (outhouses, people, shut_down): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM guilds WHERE id <> 0),
                (SELECT count(*) FROM users WHERE NOT is_bot),
                (SELECT count(*) FROM guilds WHERE shut_down_at IS NOT NULL)",
    )
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({
        "outhouses": items,
        "matching": matching,
        "page": page,
        "pages": ((matching + PAGE_SIZE - 1) / PAGE_SIZE).max(1),
        "page_size": PAGE_SIZE,
        "total_outhouses": outhouses,
        "total_people": people,
        "total_shut_down": shut_down,
    })))
}

async fn load_row(st: &AppState, guild_id: i64) -> AppResult<Row> {
    sqlx::query_as::<_, Row>(&format!("{ROW_SQL} AND g.id = $1"))
        .bind(guild_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    id: i64,
    username: String,
    display_name: String,
    age_bracket: String,
    is_bot: bool,
    suspended: bool,
    joined_at: DateTime<Utc>,
    roles: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: i64,
    name: String,
    kind: i16,
    messages: i64,
    last_message_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ReportRow {
    id: i64,
    category: String,
    status: String,
    created_at: DateTime<Utc>,
}

async fn detail_json(st: &AppState, guild_id: i64) -> AppResult<Value> {
    let r = load_row(st, guild_id).await?;
    let mut out = row_json(st, &r, online_in(st, guild_id).await?);
    let members = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id, u.username, u.display_name, u.age_bracket, u.is_bot,
                COALESCE(u.suspended_until > now(), false) AS suspended, m.joined_at,
                ARRAY(SELECT ro.name FROM member_roles mr JOIN roles ro ON ro.id = mr.role_id
                      WHERE mr.guild_id = m.guild_id AND mr.user_id = m.user_id
                      ORDER BY ro.position DESC) AS roles
         FROM guild_members m JOIN users u ON u.id = m.user_id
         WHERE m.guild_id = $1
         ORDER BY u.is_bot, lower(u.display_name), u.id
         LIMIT 500",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let channels = sqlx::query_as::<_, ChannelRow>(
        "SELECT c.id, c.name, c.kind,
                (SELECT count(*) FROM messages m WHERE m.channel_id = c.id) AS messages,
                (SELECT max(m.created_at) FROM messages m WHERE m.channel_id = c.id) AS last_message_at
         FROM channels c WHERE c.guild_id = $1
         ORDER BY c.kind, c.position, c.id",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let reports = sqlx::query_as::<_, ReportRow>(
        "SELECT id, category, status, created_at FROM reports
         WHERE guild_id = $1 ORDER BY (status = 'closed'), priority, created_at DESC LIMIT 50",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?;
    let police_links: i64 = live_police_links(st, guild_id).await?;
    out["members_list"] = json!(members
        .iter()
        .map(|m| json!({
            "id": m.id.to_string(),
            "username": m.username,
            "display_name": m.display_name,
            "age_bracket": m.age_bracket,
            "bot": m.is_bot,
            "suspended": m.suspended,
            "joined_at": m.joined_at,
            "roles": m.roles,
            "owner": m.id == r.owner_id,
        }))
        .collect::<Vec<_>>());
    out["channels_list"] = json!(channels
        .iter()
        .map(|c| json!({
            "id": c.id.to_string(),
            "name": c.name,
            "kind": c.kind,
            "messages": c.messages,
            "last_message_at": c.last_message_at,
        }))
        .collect::<Vec<_>>());
    out["reports"] = json!(reports
        .iter()
        .map(|x| json!({
            "id": x.id.to_string(),
            "category": x.category,
            "category_label": super::safety::category_label(&x.category),
            "status": x.status,
            "created_at": x.created_at,
        }))
        .collect::<Vec<_>>());
    out["member_snapshot"] = r.member_snapshot.clone().unwrap_or_else(|| json!([]));
    out["log"] = json!(guild_log(st, guild_id, 50).await?);
    out["live_police_links"] = json!(police_links);
    Ok(out)
}

async fn live_police_links(st: &AppState, guild_id: i64) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM police_links p JOIN reports r ON r.id = p.report_id
         WHERE r.guild_id = $1 AND p.package_file IS NOT NULL AND p.revoked_at IS NULL
           AND p.expires_at > now() AND p.downloads < p.max_downloads",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?)
}

/// `GET /api/safety/outhouses/{id}`
pub async fn get(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    Ok(Json(detail_json(&st, guild_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub before: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    author_id: i64,
    author_name: String,
    author_username: String,
    content: String,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    safety_hidden: bool,
}

/// `GET /api/safety/outhouses/{id}/channels/{channel}/messages?before=`:
/// a read-only look at a channel's history without joining (logged).
/// Messages held for a safety review stay hidden, and files are only links
/// (the file server still refuses quarantined and held ones).
pub async fn channel_history(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path((guild_id, channel_id)): Path<(i64, i64)>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let (name, guild_name): (String, String) = sqlx::query_as(
        "SELECT c.name, g.name FROM channels c JOIN guilds g ON g.id = c.guild_id
         WHERE c.id = $1 AND c.guild_id = $2",
    )
    .bind(channel_id)
    .bind(guild_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let before: Option<i64> = q.before.as_deref().and_then(|b| b.parse().ok());
    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT m.id, m.author_id, COALESCE(gm.nickname, u.display_name) AS author_name,
                u.username AS author_username, m.content, m.created_at, m.edited_at, m.safety_hidden
         FROM messages m JOIN users u ON u.id = m.author_id
         LEFT JOIN guild_members gm ON gm.guild_id = $2 AND gm.user_id = m.author_id
         WHERE m.channel_id = $1 AND ($3::BIGINT IS NULL OR m.id < $3)
         ORDER BY m.id DESC LIMIT 50",
    )
    .bind(channel_id)
    .bind(guild_id)
    .bind(before)
    .fetch_all(&st.db)
    .await?;
    let ids: Vec<i64> = rows
        .iter()
        .filter(|r| !r.safety_hidden)
        .map(|r| r.id)
        .collect();
    let files: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT message_id, storage_key, filename FROM attachments WHERE message_id = ANY($1) ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&st.db)
    .await?;
    if before.is_none() {
        log_for_guild(
            &st,
            Some(user.id),
            "outhouse_view",
            guild_id,
            None,
            format!("Looked at #{name} in {guild_name} (read-only, as the site owner)"),
        )
        .await;
    }
    let messages: Vec<Value> = rows
        .iter()
        .rev()
        .map(|m| {
            json!({
                "id": m.id.to_string(),
                "author_id": m.author_id.to_string(),
                "author_name": m.author_name,
                "author_username": m.author_username,
                "content": if m.safety_hidden { String::new() } else { m.content.clone() },
                "held": m.safety_hidden,
                "created_at": m.created_at,
                "edited_at": m.edited_at,
                "files": files.iter().filter(|(mid, _, _)| *mid == m.id).map(|(_, key, filename)| json!({
                    "filename": filename,
                    "url": super::messages::attachment_url(key, filename),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({
        "channel": { "id": channel_id.to_string(), "name": name },
        "messages": messages,
        "has_more": rows.len() as i64 == 50,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub suspend_owner: bool,
    #[serde(default)]
    pub confirm_name: Option<String>,
    #[serde(default)]
    pub family: Option<bool>,
    /// Restoring after members were removed: add them back (default yes).
    #[serde(default)]
    pub readd: Option<bool>,
}

/// How long members keep a shut-down outhouse on their rail (closed) before
/// they're removed: `SHUTDOWN_GRACE_DAYS` (default 7; 0 = straight away).
pub fn grace_days() -> f64 {
    std::env::var("SHUTDOWN_GRACE_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0)
        .map(|d| d.min(365.0))
        .unwrap_or(7.0)
}

fn day_label(at: DateTime<Utc>) -> String {
    at.format("%-d %B %Y").to_string()
}

/// After the grace period: snapshot who was in it (kept as evidence), then
/// remove everyone (their rail and folders lose it). Returns how many.
pub async fn remove_members(st: &AppState, guild_id: i64, actor: Option<i64>) -> AppResult<usize> {
    let snapshot: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'user_id', u.id::TEXT, 'username', u.username, 'display_name', u.display_name,
                    'age_bracket', u.age_bracket, 'bot', u.is_bot, 'joined_at', m.joined_at,
                    'role_ids', ARRAY(SELECT mr.role_id::TEXT FROM member_roles mr
                                      WHERE mr.guild_id = m.guild_id AND mr.user_id = m.user_id),
                    'roles', ARRAY(SELECT ro.name FROM member_roles mr JOIN roles ro ON ro.id = mr.role_id
                                   WHERE mr.guild_id = m.guild_id AND mr.user_id = m.user_id
                                   ORDER BY ro.position DESC))
                ORDER BY m.joined_at), '[]'::jsonb)
         FROM guild_members m JOIN users u ON u.id = m.user_id WHERE m.guild_id = $1",
    )
    .bind(guild_id)
    .fetch_one(&st.db)
    .await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM guild_members WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_all(&st.db)
        .await?;
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "UPDATE guilds SET members_removed_at = now(),
                member_snapshot = CASE WHEN jsonb_array_length($2) > 0 OR member_snapshot IS NULL
                                       THEN $2 ELSE member_snapshot END
         WHERE id = $1",
    )
    .bind(guild_id)
    .bind(&snapshot)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM guild_members WHERE guild_id = $1")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    for id in &ids {
        super::voice::clear_voice(st, *id, Some(guild_id));
        st.dispatch(Audience::User(*id), ServerEvent::GuildDelete { guild_id });
    }
    let name: String = sqlx::query_scalar("SELECT name FROM guilds WHERE id = $1")
        .bind(guild_id)
        .fetch_one(&st.db)
        .await
        .unwrap_or_default();
    log_for_guild(
        st,
        actor,
        "outhouse_members_removed",
        guild_id,
        None,
        format!(
            "The grace period for {name} ended: {} member{} removed (who they were is kept)",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        ),
    )
    .await;
    Ok(ids.len())
}

/// Every few minutes: remove members of outhouses whose grace period ended.
pub fn spawn_remover(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            tick.tick().await;
            let due: Vec<i64> = sqlx::query_scalar(
                "SELECT id FROM guilds WHERE shut_down_at IS NOT NULL AND members_removed_at IS NULL
                   AND members_remove_at <= now()",
            )
            .fetch_all(&st.db)
            .await
            .unwrap_or_default();
            for id in due {
                if let Err(e) = remove_members(&st, id, None).await {
                    tracing::warn!(error = %e, guild_id = id, "couldn't remove a shut-down outhouse's members");
                }
            }
        }
    });
}

/// `POST /api/safety/outhouses/{id}/remove-members-now`: ends the grace
/// period now. Only exists with `OUTHOUSE_TEST_HOOKS=1` (the tests).
pub async fn remove_members_now(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<Value>> {
    if std::env::var("OUTHOUSE_TEST_HOOKS").ok().as_deref() != Some("1") {
        return Err(AppError::NotFound);
    }
    require_owner(&st, &user).await?;
    let r = load_row(&st, guild_id).await?;
    if r.shut_down_at.is_none() || r.members_removed_at.is_some() {
        return Err(AppError::BadRequest("Nothing to remove.".into()));
    }
    let n = remove_members(&st, guild_id, None).await?;
    Ok(Json(json!({ "removed": n })))
}

async fn member_ids(st: &AppState, guild_id: i64) -> AppResult<Vec<(i64, bool)>> {
    Ok(sqlx::query_as(
        "SELECT m.user_id, u.is_bot FROM guild_members m JOIN users u ON u.id = m.user_id
         WHERE m.guild_id = $1",
    )
    .bind(guild_id)
    .fetch_all(&st.db)
    .await?)
}

/// `POST /api/safety/outhouses/{id}/actions`
/// `{action: "shut_down", reason, suspend_owner?}` · `{action: "restore"}` ·
/// `{action: "delete", confirm_name}` · `{action: "family", family}`
pub async fn action(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Json(req): Json<ActionRequest>,
) -> AppResult<Json<Value>> {
    require_owner(&st, &user).await?;
    let r = load_row(&st, guild_id).await?;
    let me = user.id;
    match req.action.as_str() {
        "family" => {
            let family = req.family.ok_or_else(|| {
                AppError::BadRequest("Say whether it's a family outhouse.".into())
            })?;
            sqlx::query("UPDATE guilds SET family = $2 WHERE id = $1")
                .bind(guild_id)
                .bind(family)
                .execute(&st.db)
                .await?;
            log_for_guild(
                &st,
                Some(me),
                "family",
                guild_id,
                None,
                format!(
                    "{} {} a family outhouse",
                    r.name,
                    if family { "is now" } else { "is no longer" }
                ),
            )
            .await;
        }
        "shut_down" => {
            if r.shut_down_at.is_some() {
                return Err(AppError::BadRequest("It's already shut down.".into()));
            }
            let reason: String = req
                .reason
                .as_deref()
                .unwrap_or("")
                .trim()
                .chars()
                .take(MAX_REASON)
                .collect();
            if reason.is_empty() {
                return Err(AppError::BadRequest(
                    "Write why it's being shut down (only you see this).".into(),
                ));
            }
            if req.suspend_owner && r.owner_id == me {
                return Err(AppError::BadRequest("You own this outhouse.".into()));
            }
            let remove_at: DateTime<Utc> = sqlx::query_scalar(
                "UPDATE guilds SET shut_down_at = now(), shut_down_by = $2, shut_down_reason = $3,
                        members_remove_at = now() + make_interval(secs => $4::FLOAT8),
                        members_removed_at = NULL
                 WHERE id = $1 RETURNING members_remove_at",
            )
            .bind(guild_id)
            .bind(me)
            .bind(&reason)
            .bind(grace_days() * 86400.0)
            .fetch_one(&st.db)
            .await?;
            let members = member_ids(&st, guild_id).await?;
            for (id, _) in &members {
                super::voice::clear_voice(&st, *id, Some(guild_id));
            }
            st.dispatch(
                Audience::Guild(guild_id),
                ServerEvent::GuildShutDown {
                    guild_id,
                    name: r.name.clone(),
                    remove_at,
                },
            );
            log_for_guild(
                &st,
                Some(me),
                "outhouse_shut_down",
                guild_id,
                Some(r.owner_id),
                format!(
                    "Shut down {} ({} members; they're removed on {}). Reason: {reason}",
                    r.name,
                    members.len(),
                    day_label(remove_at)
                ),
            )
            .await;
            // A neutral note to everyone in it.
            for (id, bot) in &members {
                if *bot || *id == me {
                    continue;
                }
                if let Err(e) = super::safety::safety_note(
                    &st,
                    *id,
                    format!(
                        "**{}** has been shut down by the server owner. Its channels can't be used any more, and it will be removed from your list on {}. You can leave it before then.",
                        r.name,
                        day_label(remove_at)
                    ),
                )
                .await
                {
                    tracing::warn!(error = %e, "couldn't tell a member about a shut-down outhouse");
                }
            }
            if req.suspend_owner {
                super::safety::suspend_pending_from_outhouses(&st, me, r.owner_id).await?;
            }
        }
        "restore" => {
            if r.shut_down_at.is_none() {
                return Err(AppError::BadRequest("It isn't shut down.".into()));
            }
            let removed = r.members_removed_at.is_some();
            let mut readded = 0usize;
            let mut skipped = 0usize;
            sqlx::query(
                "UPDATE guilds SET shut_down_at = NULL, shut_down_by = NULL, shut_down_reason = '',
                        members_remove_at = NULL, members_removed_at = NULL
                 WHERE id = $1",
            )
            .bind(guild_id)
            .execute(&st.db)
            .await?;
            if removed && req.readd.unwrap_or(true) {
                let snapshot = r.member_snapshot.clone().unwrap_or_else(|| json!([]));
                for m in snapshot.as_array().cloned().unwrap_or_default() {
                    let Some(uid) = m["user_id"].as_str().and_then(|x| x.parse::<i64>().ok())
                    else {
                        continue;
                    };
                    // Not anyone suspended or banned since (or gone).
                    let ok: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM users u WHERE u.id = $2
                                   AND NOT COALESCE(u.suspended_until > now(), false)
                                   AND NOT EXISTS(SELECT 1 FROM bans b WHERE b.guild_id = $1 AND b.user_id = u.id))",
                    )
                    .bind(guild_id)
                    .bind(uid)
                    .fetch_one(&st.db)
                    .await?;
                    if !ok {
                        skipped += 1;
                        continue;
                    }
                    let joined: Option<DateTime<Utc>> = m["joined_at"]
                        .as_str()
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc));
                    let added = sqlx::query(
                        "INSERT INTO guild_members (guild_id, user_id, joined_at)
                         VALUES ($1, $2, COALESCE($3, now())) ON CONFLICT DO NOTHING",
                    )
                    .bind(guild_id)
                    .bind(uid)
                    .bind(joined)
                    .execute(&st.db)
                    .await?
                    .rows_affected();
                    if added == 0 {
                        continue;
                    }
                    for role in m["role_ids"].as_array().cloned().unwrap_or_default() {
                        if let Some(role_id) = role.as_str().and_then(|x| x.parse::<i64>().ok()) {
                            sqlx::query(
                                "INSERT INTO member_roles (guild_id, user_id, role_id)
                                 SELECT $1, $2, id FROM roles WHERE id = $3 AND guild_id = $1
                                 ON CONFLICT DO NOTHING",
                            )
                            .bind(guild_id)
                            .bind(uid)
                            .bind(role_id)
                            .execute(&st.db)
                            .await?;
                        }
                    }
                    readded += 1;
                }
            }
            let guild = super::guilds::fetch_guild(&st.db, guild_id).await?;
            for (id, _) in member_ids(&st, guild_id).await? {
                st.dispatch(Audience::User(id), ServerEvent::GuildCreate(guild.clone()));
                st.dispatch(Audience::User(id), ServerEvent::GuildUpdate(guild.clone()));
            }
            log_for_guild(
                &st,
                Some(me),
                "outhouse_restore",
                guild_id,
                Some(r.owner_id),
                if removed {
                    format!(
                        "Restored {}: {readded} previous member{} added back{}",
                        r.name,
                        if readded == 1 { "" } else { "s" },
                        if skipped > 0 {
                            format!(", {skipped} skipped (suspended, banned or gone)")
                        } else {
                            String::new()
                        }
                    )
                } else {
                    format!("Restored {} (within the grace period)", r.name)
                },
            )
            .await;
        }
        "delete" => {
            if r.shut_down_at.is_none() {
                return Err(AppError::BadRequest(
                    "Shut it down first. Deleting is for good.".into(),
                ));
            }
            if req.confirm_name.as_deref().map(str::trim) != Some(r.name.trim()) {
                return Err(AppError::BadRequest(
                    "Type the outhouse's name exactly to delete it.".into(),
                ));
            }
            if r.open_reports > 0 {
                return Err(AppError::Conflict(format!(
                    "{} open report{} involve{} it. Close {} first; the evidence is kept until then.",
                    r.open_reports,
                    if r.open_reports == 1 { "" } else { "s" },
                    if r.open_reports == 1 { "s" } else { "" },
                    if r.open_reports == 1 { "it" } else { "them" },
                )));
            }
            if live_police_links(&st, guild_id).await? > 0 {
                return Err(AppError::Conflict(
                    "A police download link for a report about it is still live. Wait for it to be used or revoke it first.".into(),
                ));
            }
            let members = member_ids(&st, guild_id).await?;
            sqlx::query("DELETE FROM guilds WHERE id = $1")
                .bind(guild_id)
                .execute(&st.db)
                .await?;
            for (id, _) in members {
                st.dispatch(Audience::User(id), ServerEvent::GuildDelete { guild_id });
            }
            log_for_guild(
                &st,
                Some(me),
                "outhouse_delete",
                guild_id,
                Some(r.owner_id),
                format!("Deleted {} for good (id {guild_id})", r.name),
            )
            .await;
            return Ok(Json(json!({ "deleted": true })));
        }
        _ => return Err(AppError::BadRequest("Unknown action.".into())),
    }
    Ok(Json(detail_json(&st, guild_id).await?))
}

/// `GET /api/safety/server-status`: Server status, without an outhouse.
pub async fn server_status(
    State(st): State<AppState>,
    auth: AuthUser,
) -> AppResult<Json<super::audit::ServerStatus>> {
    super::audit::server_status(State(st), auth, Path(0)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_text_is_literal() {
        assert_eq!(like_pattern("50%_off"), "%50\\%\\_off%");
        assert_eq!(like_pattern(""), "%%");
    }

    #[test]
    fn unknown_sort_is_by_name() {
        assert_eq!(sort_sql("nope"), sort_sql("name"));
        assert!(sort_sql("reports").starts_with("open_reports"));
    }
}
