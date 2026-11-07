//! The audit log and the server status line in Outhouse settings.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use outhouse_common::id::{as_string, option_as_string};
use outhouse_common::permissions as p;
use serde::{Deserialize, Serialize};

use crate::audit::ACTIONS;
use crate::auth::{AuthUser, AVATAR_URL_SQL};
use crate::error::{AppError, AppResult};
use crate::perms::member_ctx;
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditEntry {
    #[serde(with = "as_string")]
    pub id: i64,
    pub action: String,
    #[serde(with = "option_as_string")]
    pub actor_id: Option<i64>,
    pub actor_name: Option<String>,
    pub actor_avatar_url: Option<String>,
    pub target_kind: String,
    #[serde(with = "option_as_string")]
    pub target_id: Option<i64>,
    pub target_name: String,
    pub details: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ActionInfo {
    pub key: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    /// More (older) entries exist: ask again with `before` = the last ID.
    pub has_more: bool,
    pub actions: Vec<ActionInfo>,
    pub retention_days: i64,
}

#[derive(Deserialize)]
pub struct AuditQuery {
    action: Option<String>,
    /// Entries by this user, or about them.
    user: Option<String>,
    before: Option<String>,
    limit: Option<i64>,
}

fn parse_opt_id(raw: Option<&str>, what: &str) -> AppResult<Option<i64>> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s
            .parse::<i64>()
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("`{what}` must be an ID."))),
    }
}

/// `GET /api/guilds/{id}/audit-log?action=&user=&before=&limit=` (needs View
/// Audit Log or Manage Outhouse).
pub async fn list(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<AuditPage>> {
    let ctx = member_ctx(&st.db, guild_id, user.id).await?;
    if ctx.guild_perms() & (p::VIEW_AUDIT_LOG | p::MANAGE_GUILD) == 0 {
        return Err(AppError::Forbidden);
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let before = parse_opt_id(q.before.as_deref(), "before")?;
    let by_user = parse_opt_id(q.user.as_deref(), "user")?;
    let action = q
        .action
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string);
    let sql = format!(
        "SELECT a.id, a.action, a.actor_id,
                COALESCE(m.nickname, u.display_name) AS actor_name,
                {AVATAR_URL_SQL} AS actor_avatar_url,
                a.target_kind, a.target_id, a.target_name, a.details, a.created_at
         FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_id
         LEFT JOIN guild_members m ON m.guild_id = a.guild_id AND m.user_id = a.actor_id
         WHERE a.guild_id = $1
           AND ($2::BIGINT IS NULL OR a.id < $2)
           AND ($3::TEXT IS NULL OR a.action = $3)
           AND ($4::BIGINT IS NULL OR a.actor_id = $4
                OR (a.target_kind = 'member' AND a.target_id = $4))
         ORDER BY a.id DESC
         LIMIT $5"
    );
    let mut entries = sqlx::query_as::<_, AuditEntry>(&sql)
        .bind(guild_id)
        .bind(before)
        .bind(action)
        .bind(by_user)
        .bind(limit + 1)
        .fetch_all(&st.db)
        .await?;
    let has_more = entries.len() as i64 > limit;
    entries.truncate(limit as usize);
    Ok(Json(AuditPage {
        entries,
        has_more,
        actions: ACTIONS
            .iter()
            .map(|&(key, label)| ActionInfo { key, label })
            .collect(),
        retention_days: std::env::var("AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| *d > 0)
            .unwrap_or(crate::audit::DEFAULT_RETENTION_DAYS),
    }))
}

// ---- Server status ----------------------------------------------------------

/// What backup.sh writes after each run (deploy/status/backup.json).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupStatus {
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub offsite: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ServerStatus {
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub uptime_seconds: i64,
    pub db_ok: bool,
    pub db_size_bytes: Option<i64>,
    /// People connected right now (all outhouses on this server).
    pub online_users: usize,
    pub push_enabled: bool,
    pub voice_configured: bool,
    /// None when the backup status file can't be read (e.g. running on a PC).
    pub backup: Option<BackupStatus>,
}

/// Reads the server's version: `OUTHOUSE_VERSION`, else the VERSION file
/// deploy/push.ps1 bakes into the image, else the crate version.
pub fn version() -> String {
    if let Ok(v) = std::env::var("OUTHOUSE_VERSION") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    for path in ["/app/version.txt", "VERSION"] {
        if let Ok(v) = std::fs::read_to_string(path) {
            let v = v.trim();
            if !v.is_empty() {
                return v.chars().take(64).collect();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

pub async fn read_backup_status() -> Option<BackupStatus> {
    let path = std::env::var("BACKUP_STATUS_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "/app/status/backup.json".into());
    let text = tokio::fs::read_to_string(path.trim()).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// `GET /api/guilds/{id}/server-status` (needs Manage Outhouse).
pub async fn server_status(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
    Path(guild_id): Path<i64>,
) -> AppResult<Json<ServerStatus>> {
    member_ctx(&st.db, guild_id, user.id)
        .await?
        .require(p::MANAGE_GUILD)?;
    let db_size: Option<i64> = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&st.db)
        .await
        .ok();
    let online_users = st.presence.lock().expect("presence lock").len();
    Ok(Json(ServerStatus {
        version: version(),
        started_at: st.started_at,
        uptime_seconds: (Utc::now() - st.started_at).num_seconds().max(0),
        db_ok: db_size.is_some(),
        db_size_bytes: db_size,
        online_users,
        push_enabled: st.vapid.is_some(),
        voice_configured: st.config.livekit_api_key.is_some()
            && st.config.livekit_api_secret.is_some(),
        backup: read_backup_status().await,
    }))
}

/// `GET /api/health`: 200 with `{"status":"ok", ...}` when the database
/// answers, 503 otherwise. Used by deploy/healthcheck.sh and uptime monitors.
pub async fn health(State(st): State<AppState>) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let db_ok = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&st.db),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    let body = serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "db": db_ok,
        "version": version(),
        "uptime_seconds": (Utc::now() - st.started_at).num_seconds().max(0),
    });
    let code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body)).into_response()
}
