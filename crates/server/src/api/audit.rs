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
    /// The image detector: set up, online, queue length, last error.
    pub detector: serde_json::Value,
    /// Email (mail.rs): set up or what's missing, last sent, last error.
    pub email: serde_json::Value,
    /// Where the two-factor encryption key comes from: "env"
    /// (AUTH_SECRET_KEY), "file" (a key file the server made) or "missing".
    pub auth_key: &'static str,
}

/// Reads the server's version: `OUTHOUSE_VERSION`, else the VERSION file
/// deploy/push.ps1 bakes into the image, else the crate version.
/// A short fingerprint of the web client's files (the `WEB_DIR` folder,
/// without the mirrored app downloads in `updates/`). Open pages compare it
/// with the one they loaded with and offer "Refresh to update" when the
/// server has a newer client. Worked out again at most every five minutes.
pub fn web_build() -> String {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, build)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(300) {
            return build.clone();
        }
    }
    let dir = std::env::var("WEB_DIR").unwrap_or_else(|_| "web".into());
    let build = fingerprint_dir(std::path::Path::new(&dir));
    *cache = Some((Instant::now(), build.clone()));
    build
}

/// SHA-256 over the client's file names and contents, first 16 hex digits.
pub fn fingerprint_dir(dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    const KINDS: [&str; 8] = [
        "js",
        "mjs",
        "css",
        "html",
        "svg",
        "json",
        "webmanifest",
        "wasm",
    ];
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                // App downloads are mirrored here; they aren't the web client.
                if !(d == dir && entry.file_name() == "updates") {
                    stack.push(path);
                }
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| KINDS.contains(&e))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for f in files.iter().take(5000) {
        let name = f.strip_prefix(dir).unwrap_or(f).to_string_lossy();
        hash.update(name.as_bytes());
        hash.update([0u8]);
        if let Ok(bytes) = std::fs::read(f) {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
        }
    }
    hash.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

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
    // Whole-server details: only the site owner sees them, not every
    // outhouse's admins.
    let _ = guild_id;
    if !is_site_owner(&st.db, user.id).await? {
        return Err(AppError::Forbidden);
    }
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
        detector: super::detector::health(&st).await,
        email: crate::mail::status_json(),
        auth_key: crate::twofa::key_source().as_str(),
    }))
}

/// The site owner: whoever `SITE_OWNER` names (comma-separated user names
/// or ids), otherwise the first account ever made on this server.
pub async fn is_site_owner(db: &sqlx::PgPool, user_id: i64) -> AppResult<bool> {
    if let Ok(list) = std::env::var("SITE_OWNER") {
        let list: Vec<String> = list
            .split(',')
            .map(|x| x.trim().to_lowercase())
            .filter(|x| !x.is_empty())
            .collect();
        if !list.is_empty() {
            let name: Option<String> =
                sqlx::query_scalar("SELECT lower(username) FROM users WHERE id = $1")
                    .bind(user_id)
                    .fetch_optional(db)
                    .await?;
            return Ok(
                list.contains(&user_id.to_string()) || name.is_some_and(|n| list.contains(&n))
            );
        }
    }
    let first: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE NOT is_bot ORDER BY created_at, id LIMIT 1")
            .fetch_optional(db)
            .await?;
    Ok(first == Some(user_id))
}

#[derive(Serialize)]
pub struct SiteOwner {
    pub site_owner: bool,
}

/// `GET /api/users/@me/site-owner`: lets the app decide whether to show the
/// Server status page.
pub async fn site_owner(
    State(st): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<SiteOwner>> {
    Ok(Json(SiteOwner {
        site_owner: is_site_owner(&st.db, user.id).await?,
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
    let web_build = tokio::task::spawn_blocking(web_build)
        .await
        .unwrap_or_default();
    let body = serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "db": db_ok,
        "version": version(),
        "web_build": web_build,
        "uptime_seconds": (Utc::now() - st.started_at).num_seconds().max(0),
    });
    let code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::fingerprint_dir;

    #[test]
    fn web_build_changes_with_the_client_only() {
        let dir = std::env::temp_dir().join(format!("outhouse-webbuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("updates")).unwrap();
        std::fs::create_dir_all(dir.join("vendor")).unwrap();
        std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
        std::fs::write(dir.join("vendor/lib.js"), "x").unwrap();
        let first = fingerprint_dir(&dir);
        assert_eq!(first.len(), 16);
        assert_eq!(first, fingerprint_dir(&dir), "same files, same build");
        // Mirrored app downloads and other file kinds don't count.
        std::fs::write(dir.join("updates/latest.json"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "hi").unwrap();
        assert_eq!(first, fingerprint_dir(&dir));
        // A changed script does, even in a subfolder.
        std::fs::write(dir.join("vendor/lib.js"), "y").unwrap();
        let second = fingerprint_dir(&dir);
        assert_ne!(first, second);
        std::fs::write(dir.join("style.css"), "body{}").unwrap();
        assert_ne!(second, fingerprint_dir(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
